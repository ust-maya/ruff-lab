//! Soft CPU-set pinning for hybrid and X3D-class hosts.
//!
//! * `VICTORY_FAST_CORES=1`: pin frame-critical GPU threads to the highest
//!   efficiency class (`SetThreadSelectedCpuSets` only; no process mask).
//! * `VICTORY_CACHE_CORES=1`: on uniform efficiency classes, pin to the
//!   largest last-level cache complex (Ryzen X3D CCD).
//! * `VICTORY_DISK_CORES=slow`: pin dstorage workers to lower-efficiency sets.
//!
//! All flags default off (old OS scheduling). Homogeneous CPUs stay no-ops.

use std::ffi::OsStr;
use std::sync::OnceLock;

/// Splits `(cpu_set_id, efficiency_class)` into `(fast_ids, slow_ids)` when
/// at least two efficiency classes are present.
pub fn partition_cpu_sets(entries: &[(u32, u8)]) -> Option<(Vec<u32>, Vec<u32>)> {
    if entries.is_empty() {
        return None;
    }
    let mut min_eff = u8::MAX;
    let mut max_eff = u8::MIN;
    for &(_, eff) in entries {
        min_eff = min_eff.min(eff);
        max_eff = max_eff.max(eff);
    }
    if min_eff == max_eff {
        return None;
    }
    let mut fast = Vec::new();
    let mut slow = Vec::new();
    for &(id, eff) in entries {
        if eff == max_eff {
            fast.push(id);
        } else {
            slow.push(id);
        }
    }
    Some((fast, slow))
}

/// Sets on the largest last-level cache when another cache is smaller and
/// the largest serves at least eight sets (X3D CCD split).
pub fn fast_by_cache(sets: &[(u32, u8)], l3: &[(u32, u32)]) -> Vec<u32> {
    let size = |id: u32| l3.iter().find(|s| s.0 == id).map_or(0, |s| s.1);
    let sizes: Vec<u32> = sets.iter().map(|s| size(s.0)).collect();
    let (Some(&most), Some(&least)) = (sizes.iter().max(), sizes.iter().min()) else {
        return Vec::new();
    };
    if least == 0 || least == most {
        return Vec::new();
    }
    let fast: Vec<u32> = sets.iter().filter(|s| size(s.0) == most).map(|s| s.0).collect();
    if fast.len() < 8 {
        Vec::new()
    } else {
        fast
    }
}

fn fast_cores_from(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| v.to_str().is_some_and(|s| s.trim() == "1"))
}

fn cache_cores_from(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| v.to_str().is_some_and(|s| s.trim() == "1"))
}

fn disk_cores_slow_from(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| v.to_str().is_some_and(|s| s.trim().eq_ignore_ascii_case("slow")))
}

/// `VICTORY_FAST_CORES=1` enables P-core / high-efficiency pinning.
pub fn fast_cores_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| fast_cores_from(std::env::var_os("VICTORY_FAST_CORES").as_deref()))
}

/// `VICTORY_CACHE_CORES=1` enables largest-L3 pinning on uniform CPUs.
pub fn cache_cores_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| cache_cores_from(std::env::var_os("VICTORY_CACHE_CORES").as_deref()))
}

/// `VICTORY_DISK_CORES=slow` keeps storage workers off fast cores.
pub fn disk_cores_slow() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| disk_cores_slow_from(std::env::var_os("VICTORY_DISK_CORES").as_deref()))
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
    use windows_sys::Win32::System::SystemInformation::{
        CpuSetInformation, GetLogicalProcessorInformationEx, GetSystemCpuSetInformation,
        RelationCache, SYSTEM_CPU_SET_INFORMATION, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, SetThreadSelectedCpuSets};

    use super::{
        cache_cores_enabled, disk_cores_slow, fast_by_cache, fast_cores_enabled, partition_cpu_sets,
    };

    #[derive(Clone, Copy)]
    struct CpuSetEntry {
        id: u32,
        efficiency: u8,
        group: u16,
        logical_processor: u8,
    }

    pub(super) struct Topology {
        pub(super) fast_ids: Vec<u32>,
        pub(super) slow_ids: Vec<u32>,
        /// True when sets were split by efficiency class (hybrid CPU).
        pub(super) by_class: bool,
    }

    fn query_system_cpu_sets() -> Option<Vec<CpuSetEntry>> {
        let mut needed: u32 = 0;
        // SAFETY: null buffer with length 0 queries the required byte size.
        let initial_ok = unsafe {
            GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut needed, GetCurrentProcess(), 0)
        };
        if initial_ok != 0 {
            return None;
        }
        // SAFETY: reads thread-local Win32 error immediately after the failed call.
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || needed == 0 {
            return None;
        }

        const MAX_RETRIES: usize = 4;
        for _ in 0..MAX_RETRIES {
            let words = usize::try_from(needed).ok()?.div_ceil(size_of::<u64>());
            let mut buf = vec![0u64; words];
            let cap = u32::try_from(buf.len().saturating_mul(size_of::<u64>())).ok()?;
            let mut returned: u32 = 0;
            // SAFETY: `buf` is 8-byte aligned and writable for `cap` bytes.
            let ok = unsafe {
                GetSystemCpuSetInformation(
                    buf.as_mut_ptr().cast::<SYSTEM_CPU_SET_INFORMATION>(),
                    cap,
                    &mut returned,
                    GetCurrentProcess(),
                    0,
                )
            };
            if ok != 0 {
                let used = (returned as usize).min(cap as usize);
                let base = buf.as_ptr().cast::<u8>();
                let mut entries = Vec::new();
                let mut offset = 0usize;
                while offset + size_of::<u32>() <= used {
                    // SAFETY: `offset + 4 <= used`, within `buf`.
                    let entry_size =
                        unsafe { base.add(offset).cast::<u32>().read_unaligned() } as usize;
                    if entry_size == 0 || offset.saturating_add(entry_size) > used {
                        break;
                    }
                    if entry_size >= size_of::<SYSTEM_CPU_SET_INFORMATION>() {
                        // SAFETY: `offset + entry_size <= used` and size covers the struct.
                        let info = unsafe {
                            base.add(offset)
                                .cast::<SYSTEM_CPU_SET_INFORMATION>()
                                .read_unaligned()
                        };
                        if info.Type == CpuSetInformation {
                            // SAFETY: `Type == CpuSetInformation` selects the CpuSet union arm.
                            let cs = unsafe { info.Anonymous.CpuSet };
                            entries.push(CpuSetEntry {
                                id: cs.Id,
                                efficiency: cs.EfficiencyClass,
                                group: cs.Group,
                                logical_processor: cs.LogicalProcessorIndex,
                            });
                        }
                    }
                    offset += entry_size;
                }
                return Some(entries);
            }
            // SAFETY: reads thread-local Win32 error immediately after the failed call.
            if unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER && returned > cap {
                needed = returned;
                continue;
            }
            return None;
        }
        None
    }

    fn l3_of_sets(entries: &[CpuSetEntry]) -> Vec<(u32, u32)> {
        let mut by_processor = [0u32; 64];
        let mut bytes = 0u32;
        // SAFETY: size query, then fill a buffer of that size.
        unsafe { GetLogicalProcessorInformationEx(RelationCache, std::ptr::null_mut(), &mut bytes) };
        if bytes == 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8).max(1)];
        // SAFETY: buffer holds `bytes` bytes, 8-byte aligned for the records.
        let ok = unsafe {
            GetLogicalProcessorInformationEx(RelationCache, buffer.as_mut_ptr().cast(), &mut bytes)
        };
        let mut at = 0usize;
        while ok != 0 && at + size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= bytes as usize {
            // SAFETY: a record inside the buffer; each names its own size.
            let info = unsafe {
                &*(buffer.as_ptr().cast::<u8>().add(at).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>())
            };
            if info.Size == 0 {
                break;
            }
            if info.Relationship == RelationCache {
                // SAFETY: RelationCache selects the Cache union arm.
                let cache = unsafe { &info.Anonymous.Cache };
                let mask = unsafe { cache.Anonymous.GroupMask };
                if mask.Group == 0 {
                    for (p, size) in by_processor.iter_mut().enumerate() {
                        if mask.Mask >> p & 1 != 0 {
                            *size = (*size).max(cache.CacheSize);
                        }
                    }
                }
            }
            at += info.Size as usize;
        }
        let mut sizes = Vec::new();
        for e in entries {
            if e.group == 0
                && let Some(&size) = by_processor.get(e.logical_processor as usize)
            {
                sizes.push((e.id, size));
            }
        }
        sizes
    }

    pub(super) fn topology() -> Option<&'static Topology> {
        static TOPOLOGY: OnceLock<Option<Topology>> = OnceLock::new();
        TOPOLOGY
            .get_or_init(|| {
                if !fast_cores_enabled() && !cache_cores_enabled() && !disk_cores_slow() {
                    return None;
                }
                let entries = query_system_cpu_sets()?;
                let raw: Vec<(u32, u8)> = entries.iter().map(|e| (e.id, e.efficiency)).collect();
                if let Some((fast_ids, slow_ids)) = partition_cpu_sets(&raw) {
                    tracing::info!(
                        fast = fast_ids.len(),
                        slow = slow_ids.len(),
                        fast_enabled = fast_cores_enabled(),
                        disk_slow = disk_cores_slow(),
                        "hybrid CPU topology detected"
                    );
                    return Some(Topology {
                        fast_ids,
                        slow_ids,
                        by_class: true,
                    });
                }
                if !cache_cores_enabled() {
                    return None;
                }
                let sets: Vec<(u32, u8)> = entries.iter().map(|e| (e.id, e.efficiency)).collect();
                let l3 = l3_of_sets(&entries);
                let fast_ids = fast_by_cache(&sets, &l3);
                if fast_ids.is_empty() {
                    return None;
                }
                let slow_ids: Vec<u32> = sets
                    .iter()
                    .map(|s| s.0)
                    .filter(|id| !fast_ids.contains(id))
                    .collect();
                tracing::info!(
                    fast = fast_ids.len(),
                    slow = slow_ids.len(),
                    "largest-L3 CPU sets (VICTORY_CACHE_CORES)"
                );
                Some(Topology {
                    fast_ids,
                    slow_ids,
                    by_class: false,
                })
            })
            .as_ref()
    }

    pub(super) fn pin_current_thread(role: &str, tier: &str, ids: &[u32]) -> bool {
        let Ok(count) = u32::try_from(ids.len()) else {
            return false;
        };
        if count == 0 {
            return false;
        }
        // SAFETY: current thread handle; `ids` valid for `count` u32s.
        let ok = unsafe { SetThreadSelectedCpuSets(GetCurrentThread(), ids.as_ptr(), count) } != 0;
        if ok {
            tracing::debug!(role, tier, cores = count, "pinned thread to CPU sets");
        } else {
            // SAFETY: reads thread-local Win32 error immediately after the failed call.
            let err = unsafe { GetLastError() };
            tracing::warn!(role, tier, error = err, "SetThreadSelectedCpuSets failed");
        }
        ok
    }
}

/// Pins the calling thread to fast CPU sets when enabled and topology exists.
pub fn keep_on_fast_cores(role: &str) -> bool {
    #[cfg(windows)]
    {
        // Hybrid path needs FAST_CORES; uniform X3D path needs CACHE_CORES.
        if !fast_cores_enabled() && !cache_cores_enabled() {
            return false;
        }
        let Some(topo) = imp::topology() else {
            return false;
        };
        if topo.fast_ids.is_empty() {
            return false;
        }
        if topo.by_class {
            if !fast_cores_enabled() {
                return false;
            }
        } else if !cache_cores_enabled() {
            return false;
        }
        imp::pin_current_thread(role, "fast", &topo.fast_ids)
    }
    #[cfg(not(windows))]
    {
        let _ = role;
        false
    }
}

/// Pins the calling thread to slower CPU sets when `VICTORY_DISK_CORES=slow`.
pub fn keep_on_slow_cores(role: &str) -> bool {
    #[cfg(windows)]
    {
        if !disk_cores_slow() {
            return false;
        }
        let Some(topo) = imp::topology() else {
            return false;
        };
        if topo.slow_ids.is_empty() {
            return false;
        }
        imp::pin_current_thread(role, "slow", &topo.slow_ids)
    }
    #[cfg(not(windows))]
    {
        let _ = role;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_cpu_sets_handles_empty_and_uniform() {
        assert_eq!(partition_cpu_sets(&[]), None);
        assert_eq!(partition_cpu_sets(&[(0x100, 0)]), None);
        assert_eq!(
            partition_cpu_sets(&[(0x100, 1), (0x101, 1), (0x102, 1)]),
            None
        );
    }

    #[test]
    fn partition_cpu_sets_splits_hybrid_and_multi_tier() {
        let mut entries: Vec<(u32, u8)> = (0..12).map(|i| (0x100 + i, 1)).collect();
        entries.extend((12..16).map(|i| (0x100 + i, 0)));
        let (fast, slow) = partition_cpu_sets(&entries).expect("hybrid");
        assert_eq!(fast, (0..12).map(|i| 0x100 + i).collect::<Vec<_>>());
        assert_eq!(slow, (12..16).map(|i| 0x100 + i).collect::<Vec<_>>());

        let three_tier = [(10, 0), (11, 2), (12, 1), (13, 2)];
        let (fast, slow) = partition_cpu_sets(&three_tier).expect("3-tier");
        assert_eq!(fast, vec![11, 13]);
        assert_eq!(slow, vec![10, 12]);
    }

    #[test]
    fn fast_by_cache_only_where_caches_differ() {
        const MB: u32 = 1 << 20;
        let sets: Vec<(u32, u8)> = (0..32).map(|i| (256 + i, 0)).collect();
        let l3: Vec<(u32, u32)> = (0..32)
            .map(|i| (256 + i, if i < 16 { 96 * MB } else { 32 * MB }))
            .collect();
        assert_eq!(fast_by_cache(&sets, &l3), (256..272).collect::<Vec<_>>());
        let even: Vec<(u32, u32)> = (0..32).map(|i| (256 + i, 32 * MB)).collect();
        assert!(fast_by_cache(&sets, &even).is_empty());
        let few: Vec<(u32, u32)> = (0..32)
            .map(|i| (256 + i, if i < 4 { 64 * MB } else { 32 * MB }))
            .collect();
        assert!(fast_by_cache(&sets, &few).is_empty());
    }

    #[test]
    fn env_var_parsing_defaults_off() {
        assert!(!fast_cores_from(None));
        assert!(!fast_cores_from(Some(OsStr::new(""))));
        assert!(!fast_cores_from(Some(OsStr::new("0"))));
        assert!(fast_cores_from(Some(OsStr::new("1"))));

        assert!(!cache_cores_from(None));
        assert!(cache_cores_from(Some(OsStr::new("1"))));

        assert!(!disk_cores_slow_from(None));
        assert!(!disk_cores_slow_from(Some(OsStr::new("fast"))));
        assert!(disk_cores_slow_from(Some(OsStr::new("slow"))));
        assert!(disk_cores_slow_from(Some(OsStr::new(" SLOW "))));
    }
}
