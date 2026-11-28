//! Descriptor heaps: zeroed memory, with handles that are plain addresses
//! (CPU and GPU alike, as on the console).
//!
//! Descriptor sizes: views 32 bytes and samplers 16, the size of RDNA 2's
//! image and sampler descriptors, so a title that copies descriptors itself
//! moves whole ones. Render target and depth views are ours alone; 128 bytes
//! leaves room for whatever stride a title assumes.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::object::{self, DESCRIPTOR_HEAP, E_INVALIDARG, HRESULT, Iid, Object, Table, This, guid};
use crate::resource::{Memory, PAGEABLE_IIDS};

#[cfg(windows)]
pub(crate) mod host;

const HEAP_IIDS: [Iid; 4] = [
    guid(
        0x8efb471d,
        0x616c,
        0x4f49,
        [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeapDesc {
    kind: u32,
    count: u32,
    flags: u32,
    nodes: u32,
}

/// Newer titles (the title) pass the description followed by
/// memory of their own for the descriptors: an address and its size in
/// bytes, or a null address to have the heap allocate (inferred from the
/// title's call sites; older titles pass the plain description).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Placement {
    address: u64,
    size: u64,
}

#[repr(C)]
pub(crate) struct Heap {
    object: Object,
    desc: HeapDesc,
    /// The first descriptor: in `memory`, or in the title's own memory.
    pub(crate) start: u64,
    memory: Option<Memory>,
    /// Shader-visible view and sampler heaps get a host heap with the same
    /// descriptor order when a draw first uses them (decision 0007, step
    /// 3); slots written before wait in `pending`.
    #[cfg(windows)]
    host: std::sync::OnceLock<Option<host::HostHeap>>,
    #[cfg(windows)]
    pending: std::sync::Mutex<Vec<u32>>,
    /// Changed pages of a placed shader-visible heap. Some titles copy
    /// descriptor records into it as bytes, without a descriptor API call.
    #[cfg(windows)]
    written: Option<crate::watch::Tracked>,
}

pub(crate) fn heap<'a>(this: This) -> &'a Heap {
    // SAFETY: called from descriptor heap methods only.
    unsafe { &*this.cast::<Heap>() }
}

impl Heap {
    /// Whether this is a sampler heap.
    pub(crate) fn samplers(&self) -> bool {
        self.desc.kind == SAMPLERS
    }

    /// The host heap, made on first use from what the title wrote so far.
    /// Made late because a host view heap the size of the title's
    /// costs over 100 MB of commit, which its boot needs for its page pool.
    #[cfg(windows)]
    pub(crate) fn host(&self) -> Option<&host::HostHeap> {
        if let Some(h) = self.host.get() {
            return h.as_ref();
        }
        if self.desc.flags & SHADER_VISIBLE == 0 || !matches!(self.desc.kind, VIEWS_KIND | SAMPLERS)
        {
            return None;
        }
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        // A recorder can take the fast path as soon as the OnceLock is
        // initialized. Publish only after replaying the queued writes.
        self.host
            .get_or_init(|| {
                let step = u64::from(increment(self.desc.kind)?);
                let h = host::HostHeap::new(self.samplers(), self.desc.count)?;
                for i in pending.drain(..) {
                    h.mirror(i, self.start + u64::from(i) * step, false);
                }
                Some(h)
            })
            .as_ref()
    }

    /// The handle of descriptor `index` of this heap.
    #[cfg(windows)]
    pub(crate) fn handle(&self, index: u32) -> Option<u64> {
        let step = u64::from(increment(self.desc.kind)?);
        (index < self.desc.count).then(|| self.start + u64::from(index) * step)
    }

    /// The descriptor index of `handle` in this heap, if it lies in it.
    pub(crate) fn index(&self, handle: u64) -> Option<u32> {
        let step = u64::from(increment(self.desc.kind)?);
        let at = handle.checked_sub(self.start)?;
        let index = at / step;
        (index < u64::from(self.desc.count)).then_some(index as u32)
    }
}

const VIEWS_KIND: u32 = 0;
const SAMPLERS: u32 = 1;
const SHADER_VISIBLE: u32 = 1;

/// Shader-visible heaps by address range, for mirroring descriptor writes:
/// start, end, heap. Uses `RwLock` so concurrent draw-recording threads never
/// serialize behind `sync_heaps` or one another (`Marvisak/VIctory#2821`).
#[cfg(windows)]
static SHADER_HEAPS: std::sync::RwLock<Vec<(u64, u64, usize)>> = std::sync::RwLock::new(Vec::new());

/// The shader-visible heap holding `handle`, and the descriptor's index.
#[cfg(windows)]
fn shader_slot<'a>(handle: u64) -> Option<(&'a Heap, u32)> {
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    let &(_, _, at) = heaps
        .iter()
        .find(|&&(start, end, _)| (start..end).contains(&handle))?;
    // SAFETY: registered heaps are live (removed when dropped).
    let heap = unsafe { &*(at as *const Heap) };
    Some((heap, heap.index(handle)?))
}

/// Makes the host descriptor for what the title wrote at `handle`, if it
/// is in a shader-visible heap.
#[cfg(windows)]
fn mirror(handle: u64) {
    let Some((heap, index)) = shader_slot(handle) else {
        return;
    };
    let mut pending = heap.pending.lock().unwrap_or_else(|e| e.into_inner());
    match heap.host.get() {
        None => pending.push(index),
        Some(host) => {
            drop(pending);
            if let Some(host) = host {
                host.mirror(index, handle, false);
            }
        }
    }
}

/// Mirrors records copied directly into placed shader-visible heaps. Calls
/// to `Create*View` are already mirrored by [`mirror`]; raw CPU copies leave
/// only write-watch pages to reveal which slots changed.
///
/// Upstream VIctory 86a5ed6 does the same and counts 1,300 to 14,400 such
/// slots a frame in the world of the title this fork works on, against a
/// few hundred views made by calls; without it those host slots stayed
/// null, or went on showing what they showed before.
/// `VICTORY_HEAP_SYNC=0`: not done, as before. [`SYNCED`] counts.
///
/// Two opt-in cuts on the same pass (`VICTORY_HEAP_SYNC_SKIP=1`,
/// `VICTORY_HEAP_SYNC_BATCH=1`; both off by default, so the reads stay one
/// page at a time). Per-draw tables dirty the same heap every frame, and
/// [`host::HostHeap::mirror_copied`] already leaves those slots to their
/// draws: the page was still read first. A failed batched read falls back
/// to the per-page read, so one unmapped page does not drop its neighbours.
#[cfg(windows)]
pub(crate) fn sync_heaps() {
    use std::sync::atomic::Ordering::Relaxed;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_SYNC").is_none_or(|v| v != "0")) {
        return;
    }
    let started = std::time::Instant::now();
    let mut pages = 0u64;
    let mut looked = 0u64;
    let mut mirrored = 0u64;
    let mut skipped = 0u64;
    let batch = heap_sync_batch();
    let skip_tabled = heap_sync_skip();
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    for &(start, end, at) in heaps.iter() {
        // SAFETY: registered heaps are live (removed when dropped under write lock).
        let heap = unsafe { &*(at as *const Heap) };
        let (Some(Some(host)), Some(written), Some(step)) =
            (heap.host.get(), &heap.written, increment(heap.desc.kind))
        else {
            continue;
        };
        let step = u64::from(step);
        // Capped host heaps have no counterparts for the title slots reserved
        // for per-draw tables; raw writes there must not be mirrored.
        let mirrored_count = heap.desc.count.min(host.count());
        let mut pages_touched: Vec<(usize, usize)> = Vec::new();
        for (range_from, range_end) in written.take(start as usize, end as usize) {
            let mut page = range_from & !(crate::watch::PAGE - 1);
            while page < range_end {
                let from = page.max(range_from);
                let to = (page + crate::watch::PAGE).min(range_end);
                pages_touched.push((from, to));
                page += crate::watch::PAGE;
            }
        }
        let reads = plan_sync_reads(&pages_touched, batch, |from, to| {
            if !skip_tabled {
                return false;
            }
            let slots = changed_slots(from, to, start as usize, step, mirrored_count);
            let first = slots.start as u32;
            let last = slots.end as u32;
            if first >= last || host.records_left_to_draws(first, last) {
                skipped += 1;
                true
            } else {
                false
            }
        });
        let process = |chunk: &[(usize, usize)]| -> (u64, u64, u64) {
            let (mut p, mut l, mut m) = (0u64, 0u64, 0u64);
            let mut page_bytes: Vec<u8> = Vec::with_capacity(batch.unwrap_or(crate::watch::PAGE));
            let mut fallback: Vec<u8> = Vec::new();
            for &(from, to) in chunk {
                let (np, nl, nm) = mirror_read(
                    host,
                    &mut page_bytes,
                    &mut fallback,
                    from,
                    to,
                    start,
                    step,
                    mirrored_count,
                );
                p += np;
                l += nl;
                m += nm;
            }
            (p, l, m)
        };
        if reads.len() >= 16 {
            let threads = std::thread::available_parallelism()
                .map_or(4, |n| (n.get() / 2).clamp(2, 6))
                .min(reads.len() / 8)
                .max(1);
            let chunk_len = reads.len().div_ceil(threads);
            let totals: Vec<(u64, u64, u64)> = std::thread::scope(|s| {
                let handles: Vec<_> = reads
                    .chunks(chunk_len)
                    .map(|c| s.spawn(move || process(c)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap_or((0, 0, 0))).collect()
            });
            for (p, l, m) in totals {
                pages += p;
                looked += l;
                mirrored += m;
            }
        } else {
            let (p, l, m) = process(&reads);
            pages += p;
            looked += l;
            mirrored += m;
        }
    }
    drop(heaps);
    for (count, n) in SYNCED.iter().zip([
        pages,
        looked,
        mirrored,
        started.elapsed().as_micros() as u64,
        skipped,
    ]) {
        count.fetch_add(n, Relaxed);
    }
}

/// One prefetched read for [`SyncJob`]: owned bytes plus the record prints
/// the worker computed in slot order (submitter only CASes/mirrors with
/// them; see [`host::HostHeap::mirror_copied_print`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyncRead {
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) bytes: Vec<u8>,
    pub(crate) prints: Vec<u64>,
}

/// One heap's planned sync reads for [`SyncJob`]: owned, sequence-keyed by
/// the submission's `STARTED` sequence at the [`SyncJob`] level. `range` is
/// `(heap_start, heap_end)` (title heap memory), `heap_at` the `Heap`
/// identity for revalidation under the submission lock (no `HostHeap`/
/// `Arc`/`HostList` reference is held across the worker), `step` bytes per
/// descriptor, `mirrored_count` the capped host count, `batch` the read
/// batching (`VICTORY_HEAP_SYNC_BATCH`), `skip` whether tabled pages are
/// skipped (`VICTORY_HEAP_SYNC_SKIP`). `peeked` is the worker's
/// non-destructive journal look (destructive `take` stays on the
/// submitter); `skipped_spans` the spans the worker skipped as all-tabled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyncHeap {
    pub(crate) heap_start: u64,
    pub(crate) heap_end: u64,
    pub(crate) heap_at: usize,
    pub(crate) step: u64,
    pub(crate) mirrored_count: u32,
    pub(crate) batch: Option<usize>,
    pub(crate) skip: bool,
    pub(crate) peeked: Vec<(usize, usize)>,
    /// Journal generation per `peeked` page (see `watch::Tracked`):
    /// a page whose generation moved by the submitter's `take` is
    /// re-read, never mirrored stale.
    pub(crate) stamps: Vec<(usize, u64)>,
    pub(crate) skipped_spans: Vec<(usize, usize)>,
    pub(crate) reads: Vec<SyncRead>,
}

/// Worker-prefetched sync plan + bytes for one submission: the pure-CPU
/// part of [`sync_heaps`] (heap snapshot, `plan_sync_reads` planning,
/// `records_left_to_draws` check, RPM reads, slicing, `record_print`).
/// The destructive `written.take`, the `copied` CAS + host-heap mutation,
/// `SYNCED` counts and retire stay on the submitter under `submitted`
/// after settle (see [`apply_sync_job`]); `snapshots`/`card_ranges` in
/// `Prepared` stay empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyncJob {
    pub(crate) heaps: Vec<SyncHeap>,
}

impl SyncJob {
    /// One heap with one empty read per range (tests only).
    #[cfg(test)]
    pub(crate) fn from_reads(reads: Vec<(usize, usize)>) -> Self {
        Self {
            heaps: vec![SyncHeap {
                peeked: reads.clone(),
                reads: reads
                    .into_iter()
                    .map(|(from, to)| SyncRead {
                        from,
                        to,
                        bytes: vec![0; to.saturating_sub(from)],
                        prints: Vec::new(),
                    })
                    .collect(),
                ..Default::default()
            }],
        }
    }
}

/// Whether any planned sync range touches `written`/`in_hand` (title
/// memory flights may still write), or a settle is landing (`settling`
/// with non-empty sync: the relay may be writing heap pages now). Pure
/// range check, for `wait_done` before and after the done-wait and the
/// submitter's final check after settle.
pub(crate) fn sync_job_touches(
    job: &SyncJob,
    written: &[(usize, usize)],
    settling: bool,
    in_hand: &[(usize, usize)],
) -> bool {
    fn overlap(a: (usize, usize), b: &[ (usize, usize)]) -> bool {
        a.0 < a.1 && b.iter().any(|&(f, t)| f < a.1 && a.0 < t)
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for heap in &job.heaps {
        ranges.extend_from_slice(&heap.peeked);
        ranges.extend(heap.reads.iter().map(|r| (r.from, r.to)));
    }
    if ranges.iter().all(|&(f, t)| f >= t) {
        return false;
    }
    if settling {
        return true;
    }
    ranges.iter().any(|&r| overlap(r, written) || overlap(r, in_hand))
}

/// [`sync_heaps`] prefetched on the `victory-prep` worker: snapshots the
/// heaps, `peek`s the journals (never `take`), plans with
/// `plan_sync_reads`, checks `records_left_to_draws`, then RPM-reads
/// (`read_into`/`plaincopy`) and slices + `record_print`s. Touches no
/// `submitted`/`CARD_WORK`/fence/event/`LINE` and holds no host references
/// across the reads; `None` when `VICTORY_HEAP_SYNC=0` (caller runs the
/// inline path, which also no-ops).
#[cfg(windows)]
pub(crate) fn prefetch_sync_job() -> Option<SyncJob> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_SYNC").is_none_or(|v| v != "0")) {
        return None;
    }
    let batch = heap_sync_batch();
    let skip_tabled = heap_sync_skip();
    struct Snap {
        heap_start: u64,
        heap_end: u64,
        heap_at: usize,
        step: u64,
        mirrored_count: u32,
        peeked: Vec<(usize, usize)>,
        stamps: Vec<(usize, u64)>,
        planned: Vec<(usize, usize)>,
        skipped_spans: Vec<(usize, usize)>,
    }
    let snaps: Vec<Snap> = {
        let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for &(start, end, at) in heaps.iter() {
            // SAFETY: registered heaps are live (removed when dropped under
            // write lock); the read lock is held for the whole snapshot.
            let heap = unsafe { &*(at as *const Heap) };
            let (Some(Some(host)), Some(written), Some(step)) =
                (heap.host.get(), &heap.written, increment(heap.desc.kind))
            else {
                continue;
            };
            let step = u64::from(step);
            let mirrored_count = heap.desc.count.min(host.count());
            let (peeked, stamps) = written.peek_stamped(start as usize, end as usize);
            let mut pages_touched: Vec<(usize, usize)> = Vec::new();
            for (range_from, range_end) in &peeked {
                let mut page = range_from & !(crate::watch::PAGE - 1);
                while page < *range_end {
                    let from = page.max(*range_from);
                    let to = (page + crate::watch::PAGE).min(*range_end);
                    pages_touched.push((from, to));
                    page += crate::watch::PAGE;
                }
            }
            let mut skipped_spans: Vec<(usize, usize)> = Vec::new();
            let planned = plan_sync_reads(&pages_touched, batch, |from, to| {
                if !skip_tabled {
                    return false;
                }
                let slots = changed_slots(from, to, start as usize, step, mirrored_count);
                let first = slots.start as u32;
                let last = slots.end as u32;
                if first >= last || host.records_left_to_draws(first, last) {
                    skipped_spans.push((from, to));
                    true
                } else {
                    false
                }
            });
            out.push(Snap {
                heap_start: start,
                heap_end: end,
                heap_at: at,
                step,
                mirrored_count,
                peeked,
                stamps,
                planned,
                skipped_spans,
            });
        }
        out
    };
    // No SHADER_HEAPS lock, no host references below: title-memory RPM only.
    let mut heaps = Vec::with_capacity(snaps.len());
    for snap in snaps {
        let mut reads = Vec::new();
        for (from, to) in snap.planned {
            reads.extend(read_pieces(from, to, snap.heap_start, snap.step, snap.mirrored_count));
        }
        heaps.push(SyncHeap {
            heap_start: snap.heap_start,
            heap_end: snap.heap_end,
            heap_at: snap.heap_at,
            step: snap.step,
            mirrored_count: snap.mirrored_count,
            batch,
            skip: skip_tabled,
            peeked: snap.peeked,
            stamps: snap.stamps,
            skipped_spans: snap.skipped_spans,
            reads,
        });
    }
    Some(SyncJob { heaps })
}

/// RPM pieces of one planned read: like [`mirror_read`]'s reads (whole
/// span, then per-page fallback) without mirroring; prints in slot order.
#[cfg(windows)]
fn read_pieces(
    from: usize,
    to: usize,
    heap_start: u64,
    step: u64,
    mirrored_count: u32,
) -> Vec<SyncRead> {
    let mut out = Vec::new();
    if from >= to {
        return out;
    }
    let mut buf = vec![0u8; to - from];
    if crate::host::read_into(from as *const u8, &mut buf) {
        let prints = prints_for(from, &buf, heap_start, step, mirrored_count);
        out.push(SyncRead { from, to, bytes: buf, prints });
        return out;
    }
    if to - from <= crate::watch::PAGE {
        return out;
    }
    let mut page = from & !(crate::watch::PAGE - 1);
    while page < to {
        let start = page.max(from);
        let end = (page + crate::watch::PAGE).min(to);
        let mut buf = vec![0u8; end - start];
        if crate::host::read_into(start as *const u8, &mut buf) {
            let prints = prints_for(start, &buf, heap_start, step, mirrored_count);
            out.push(SyncRead { from: start, to: end, bytes: buf, prints });
        }
        page += crate::watch::PAGE;
    }
    out
}

/// `record_print` of every fitting record in `bytes` (from `base`), in
/// [`changed_slots`] order (same order [`mirror_records_prefetched`]
/// pairs them).
#[cfg(windows)]
fn prints_for(base: usize, bytes: &[u8], heap_start: u64, step: u64, mirrored_count: u32) -> Vec<u64> {
    let mut prints = Vec::new();
    let slots = changed_slots(base, base + bytes.len(), heap_start as usize, step, mirrored_count);
    for index in slots {
        let handle = heap_start + index * step;
        let Some(at) = (handle as usize).checked_sub(base) else {
            continue;
        };
        let Some(record) = bytes.get(at..at + step as usize) else {
            continue;
        };
        prints.push(host::record_print(record) | 1);
    }
    prints
}

/// Whether a prefetched read still names the title bytes of the
/// submitter's `take`: every watch page under `[from, to)` must have the
/// same journal generation in the worker's `peek` and the submitter's
/// `take` (a copy-back/titles-copy landing on an already-dirty page mid-
/// prefetch advances only the generation, and the re-read below catches
/// it). Pure map comparison.
#[cfg(windows)]
fn read_stamps_match(
    peek: &std::collections::HashMap<usize, u64>,
    taken: &std::collections::HashMap<usize, u64>,
    from: usize,
    to: usize,
) -> bool {
    if from >= to {
        return false;
    }
    let page = crate::watch::PAGE;
    (from / page..=(to - 1) / page)
        .all(|p| peek.get(&p).is_some_and(|&g| taken.get(&p) == Some(&g)))
}

/// Applies a worker-prefetched [`SyncJob`] on the submitter under
/// `submitted` after settle. Returns `false` (before any destructive
/// `take`, so the caller can run [`sync_heaps`] inline byte-identical)
/// only if a skipped all-tabled span is no longer left to its draws;
/// otherwise the `take` re-asks the journal after any landing (so a
/// copy-back/shadow landing mid-prefetch is not lost) and always
/// completes mirroring for the taken ranges (prefetched bytes where the
/// span matches exactly and its pages did not move, inline RPM
/// otherwise), counting `SYNCED` like [`sync_heaps`].
#[cfg(windows)]
pub(crate) fn apply_sync_job(job: &SyncJob) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let started = std::time::Instant::now();
    let batch = heap_sync_batch();
    let skip_tabled = heap_sync_skip();
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    // Live heaps (a job heap that vanished is ignored: its journal is
    // gone with it; a live heap without a job entry is read inline below).
    struct Live {
        start: u64,
        end: u64,
        at: usize,
        host_at: *const host::HostHeap,
        step: u64,
        mirrored_count: u32,
    }
    let mut live: Vec<Live> = Vec::new();
    for &(start, end, at) in heaps.iter() {
        // SAFETY: registered heaps are live under the read lock.
        let heap = unsafe { &*(at as *const Heap) };
        let (Some(Some(host)), Some(written), Some(step)) =
            (heap.host.get(), &heap.written, increment(heap.desc.kind))
        else {
            continue;
        };
        let step = u64::from(step);
        let mirrored_count = heap.desc.count.min(host.count());
        // Skipped all-tabled spans must still be left to draws (checked
        // before any `take`, so a failure runs the inline path whose
        // journal is still virgin).
        let _ = written;
        if skip_tabled
            && let Some(entry) = job.heaps.iter().find(|e| e.heap_at == at)
        {
            for &(from, to) in &entry.skipped_spans {
                let slots = changed_slots(from, to, start as usize, step, mirrored_count);
                let first = slots.start as u32;
                let last = slots.end as u32;
                if first < last && !host.records_left_to_draws(first, last) {
                    drop(heaps);
                    return false;
                }
            }
        }
        // SAFETY: `heaps` read lock is held for the rest of this call.
        live.push(Live {
            start,
            end,
            at,
            host_at: std::ptr::from_ref::<host::HostHeap>(host),
            step,
            mirrored_count,
        });
    }
    // Destructive take + mirror (scoped per heap, as `sync_heaps`).
    let mut pages = 0u64;
    let mut looked = 0u64;
    let mut mirrored = 0u64;
    let mut skipped = 0u64;
    for i in 0..live.len() {
        // SAFETY: the heap is live under the read lock.
        let (start, end, at, host, step, mirrored_count) = {
            let l = &live[i];
            (l.start, l.end, l.at, unsafe { &*l.host_at }, l.step, l.mirrored_count)
        };
        // SAFETY: registered heaps are live (removed when dropped under write lock).
        let heap = unsafe { &*(at as *const Heap) };
        let Some(written) = &heap.written else {
            continue;
        };
        let (taken, stamps) = written.take_stamped(start as usize, end as usize);
        let mut pages_touched: Vec<(usize, usize)> = Vec::new();
        for (range_from, range_end) in &taken {
            let mut page = range_from & !(crate::watch::PAGE - 1);
            while page < *range_end {
                let from = page.max(*range_from);
                let to = (page + crate::watch::PAGE).min(*range_end);
                pages_touched.push((from, to));
                page += crate::watch::PAGE;
            }
        }
        let mut skipped_here = 0u64;
        let reads = plan_sync_reads(&pages_touched, batch, |from, to| {
            if !skip_tabled {
                return false;
            }
            let slots = changed_slots(from, to, start as usize, step, mirrored_count);
            let first = slots.start as u32;
            let last = slots.end as u32;
            if first >= last || host.records_left_to_draws(first, last) {
                skipped_here += 1;
                true
            } else {
                false
            }
        });
        skipped += skipped_here;
        let entry = job.heaps.iter().find(|e| e.heap_at == at);
        let generations: std::collections::HashMap<usize, u64> = stamps.into_iter().collect();
        let entry_stamps = entry.map_or(Vec::new(), |e| e.stamps.clone());
        let peeked: std::collections::HashMap<usize, u64> = entry_stamps.into_iter().collect();
        let process = |chunk: &[(usize, usize)]| -> (u64, u64, u64) {
            let (mut p, mut l, mut m) = (0u64, 0u64, 0u64);
            let mut page_bytes: Vec<u8> = Vec::new();
            let mut fallback: Vec<u8> = Vec::new();
            for &(from, to) in chunk {
                let unchanged = read_stamps_match(&peeked, &generations, from, to);
                if unchanged
                    && let Some(read) = entry.and_then(|e| e.reads.iter().find(|r| r.from == from && r.to == to))
                {
                    p += page_pieces(from, to, crate::watch::PAGE);
                    let (dl, dm) = mirror_records_prefetched(host, from, &read.bytes, &read.prints, start, step, mirrored_count);
                    l += dl;
                    m += dm;
                } else {
                    let (np, nl, nm) = mirror_read(host, &mut page_bytes, &mut fallback, from, to, start, step, mirrored_count);
                    p += np;
                    l += nl;
                    m += nm;
                }
            }
            (p, l, m)
        };
        if reads.len() >= 16 {
            let threads = std::thread::available_parallelism()
                .map_or(4, |n| (n.get() / 2).clamp(2, 6))
                .min(reads.len() / 8)
                .max(1);
            let chunk_len = reads.len().div_ceil(threads);
            let totals: Vec<(u64, u64, u64)> = std::thread::scope(|s| {
                let handles: Vec<_> = reads.chunks(chunk_len).map(|c| s.spawn(move || process(c))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or((0, 0, 0))).collect()
            });
            for (p, l, m) in totals {
                pages += p;
                looked += l;
                mirrored += m;
            }
        } else {
            let (p, l, m) = process(&reads);
            pages += p;
            looked += l;
            mirrored += m;
        }
    }
    drop(heaps);
    for (count, n) in SYNCED.iter().zip([
        pages,
        looked,
        mirrored,
        started.elapsed().as_micros() as u64,
        skipped,
    ]) {
        count.fetch_add(n, Relaxed);
    }
    true
}

/// Mirrors prefetched `bytes` via their worker prints (CAS + mirror only;
/// see [`host::HostHeap::mirror_copied_print`]).
#[cfg(windows)]
fn mirror_records_prefetched(
    host: &host::HostHeap,
    base: usize,
    bytes: &[u8],
    prints: &[u64],
    heap_start: u64,
    step: u64,
    mirrored_count: u32,
) -> (u64, u64) {
    let mut looked = 0u64;
    let mut mirrored = 0u64;
    let mut prints = prints.iter();
    let slots = changed_slots(base, base + bytes.len(), heap_start as usize, step, mirrored_count);
    for index in slots {
        let handle = heap_start + index * step;
        let Some(at) = (handle as usize).checked_sub(base) else {
            continue;
        };
        let Some(record) = bytes.get(at..at + step as usize) else {
            continue;
        };
        looked += 1;
        let done = match prints.next() {
            Some(&print) => host.mirror_copied_print(index as u32, handle, record, print),
            None => host.mirror_copied(index as u32, handle, record),
        };
        if done {
            mirrored += 1;
        }
    }
    (looked, mirrored)
}

/// Largest read [`sync_heaps`] issues when `VICTORY_HEAP_SYNC_BATCH` is set.
/// `None`: one dirty watch page (64 KiB, [`crate::watch::PAGE`]) per read,
/// as before. `1` is 1 MiB; any other value is a KiB count clamped to
/// 128..=4096 (two watch pages through 4 MiB). A failed read is retried
/// page by page.
#[cfg(windows)]
fn heap_sync_batch() -> Option<usize> {
    static ON: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("VICTORY_HEAP_SYNC_BATCH").ok().as_deref() {
        None | Some("0") => None,
        Some("1") => Some(1 << 20),
        Some(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .map(|kb| kb.clamp(128, 4096) << 10),
    })
}

/// Whether [`sync_heaps`] skips a dirty page whose every host slot is left
/// to its draws (`VICTORY_HEAP_SYNC_SKIP=1`; off by default).
#[cfg(windows)]
fn heap_sync_skip() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_HEAP_SYNC_SKIP").is_some_and(|v| v == "1"))
}

/// Reads `[from, to)` and mirrors the descriptor records in it.
///
/// One read of the whole span. If that fails (a page unmapped under the
/// span), each page is read alone: the page that fails is left, and the
/// others are still mirrored. Returns pages read, slots looked at, slots
/// mirrored.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn mirror_read(
    host: &host::HostHeap,
    buf: &mut Vec<u8>,
    fallback: &mut Vec<u8>,
    from: usize,
    to: usize,
    heap_start: u64,
    step: u64,
    mirrored_count: u32,
) -> (u64, u64, u64) {
    if from >= to {
        return (0, 0, 0);
    }
    buf.resize(to - from, 0);
    if crate::host::read_into(from as *const u8, buf) {
        let (looked, mirrored) = mirror_records(host, from, buf, heap_start, step, mirrored_count);
        return (page_pieces(from, to, crate::watch::PAGE), looked, mirrored);
    }
    if to - from <= crate::watch::PAGE {
        return (0, 0, 0);
    }
    let (mut pages, mut looked, mut mirrored) = (0u64, 0u64, 0u64);
    let mut page = from & !(crate::watch::PAGE - 1);
    while page < to {
        let start = page.max(from);
        let end = (page + crate::watch::PAGE).min(to);
        fallback.resize(end - start, 0);
        if crate::host::read_into(start as *const u8, fallback) {
            pages += 1;
            let (l, m) = mirror_records(host, start, fallback, heap_start, step, mirrored_count);
            looked += l;
            mirrored += m;
        }
        page += crate::watch::PAGE;
    }
    (pages, looked, mirrored)
}

/// How many `page`-sized pieces `[from, to)` covers (the frame line's page count).
fn page_pieces(from: usize, to: usize, page: usize) -> u64 {
    if from >= to || page == 0 {
        return 0;
    }
    let mut n = 0u64;
    let mut at = from & !(page - 1);
    while at < to {
        n += 1;
        at += page;
    }
    n
}

/// Mirrors descriptor records in `bytes`, which were read from `base`.
#[cfg(windows)]
fn mirror_records(
    host: &host::HostHeap,
    base: usize,
    bytes: &[u8],
    heap_start: u64,
    step: u64,
    mirrored_count: u32,
) -> (u64, u64) {
    let mut looked = 0u64;
    let mut mirrored = 0u64;
    let slots = changed_slots(base, base + bytes.len(), heap_start as usize, step, mirrored_count);
    for index in slots {
        let handle = heap_start + index * step;
        // A record that straddles the range's ends is left to the range
        // that holds its first byte.
        let Some(at) = (handle as usize).checked_sub(base) else {
            continue;
        };
        let Some(record) = bytes.get(at..at + step as usize) else {
            continue;
        };
        looked += 1;
        if host.mirror_copied(index as u32, handle, record) {
            mirrored += 1;
        }
    }
    (looked, mirrored)
}

/// Groups dirty page spans into the reads [`sync_heaps`] will issue.
///
/// `skip` drops a span (a page left entirely to per-draw tables). `batch`
/// joins adjacent kept spans up to that many bytes; spans with a gap
/// between them stay apart, so a skipped page is not read as part of its
/// neighbours. `None` keeps one read per remaining span.
pub(crate) fn plan_sync_reads(
    pages: &[(usize, usize)],
    batch: Option<usize>,
    mut skip: impl FnMut(usize, usize) -> bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(from, to) in pages {
        if from >= to || skip(from, to) {
            continue;
        }
        if let Some(limit) = batch
            && let Some(last) = out.last_mut()
        {
            let (start, end) = *last;
            if end == from && to.saturating_sub(start) <= limit {
                *last = (start, to);
                continue;
            }
        }
        out.push((from, to));
    }
    out
}

/// Since the last frame line, of [`sync_heaps`]: changed pages of placed
/// shader-visible heaps looked at, slots on them, slots whose record was
/// not the one last seen (mirrored), the microseconds it took, and pages
/// not read because every slot on them was left to its draws
/// (`VICTORY_HEAP_SYNC_SKIP=1`).
#[cfg(windows)]
pub(crate) static SYNCED: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5];

/// With the frame line: what [`sync_heaps`] did per frame over `frames`.
/// Nothing while no heap page changed and none was skipped.
#[cfg(windows)]
pub(crate) fn sync_line(frames: f64) {
    let [pages, looked, mirrored, micros, skipped] =
        [0, 1, 2, 3, 4].map(|i| SYNCED[i].swap(0, std::sync::atomic::Ordering::Relaxed));
    if pages == 0 && skipped == 0 {
        return;
    }
    tracing::info!(
        "descriptor records the title copied into its heap as bytes (VICTORY_HEAP_SYNC=0: not looked for): per frame \
         {:.1} changed heap pages, {:.0} slots looked at, {:.1} whose record was new and was mirrored, {:.0} left to the \
         draws that read them through tables (VICTORY_HEAP_SYNC_TABLES=1 mirrors those too), {:.0} pages not read \
         (VICTORY_HEAP_SYNC_SKIP=1; batched reads VICTORY_HEAP_SYNC_BATCH=1), {:.2} ms",
        pages as f64 / frames,
        looked as f64 / frames,
        mirrored as f64 / frames,
        host::COPIED_TABLED.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / frames,
        skipped as f64 / frames,
        micros as f64 / 1000.0 / frames
    );
}

/// Title descriptor slots in a changed byte range that have host counterparts.
/// `range_end` is exclusive.
#[cfg(windows)]
fn changed_slots(
    range_from: usize,
    range_end: usize,
    heap_start: usize,
    step: u64,
    mirrored_count: u32,
) -> std::ops::Range<u64> {
    let from = (range_from - heap_start) as u64;
    let to = (range_end - heap_start) as u64;
    let first = from / step;
    let last = to.div_ceil(step).min(u64::from(mirrored_count));
    first.min(last)..last
}

/// Gives back dynamic slots of the host heap behind the title heap at
/// `heap`, if that heap still exists and is the one with identity `id`
/// (`HostHeap::id`) that gave them (`kitty-core/Kitty6` `aca0877`).
#[cfg(windows)]
pub(crate) fn release_dynamic(heap: usize, id: u64, first: u32, n: u32) {
    let heaps = SHADER_HEAPS.read().unwrap_or_else(|e| e.into_inner());
    if heaps.iter().any(|&(_, _, h)| h == heap)
        // SAFETY: registered heaps are live (removed when dropped).
        && let Some(Some(host)) = unsafe { &*(heap as *const Heap) }.host.get()
    {
        host.release(id, first, n);
    }
}

#[cfg(windows)]
impl Drop for Heap {
    fn drop(&mut self) {
        let at = std::ptr::from_ref::<Heap>(self) as usize;
        let mut heaps = SHADER_HEAPS.write().unwrap_or_else(|e| e.into_inner());
        heaps.retain(|&(_, _, h)| h != at);
    }
}

/// Bytes per descriptor, by heap type (views, samplers, render targets,
/// depth).
fn increment(kind: u32) -> Option<u32> {
    [32, 16, 128, 128].get(kind as usize).copied()
}

/// Device method.
pub(crate) extern "system" fn increment_size(_: This, kind: u32) -> u32 {
    increment(kind).unwrap_or(0)
}

/// What our render target views hold: the resource and the view
/// description, turned into a host view when a command uses them.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RtvRecord {
    tag: u32,
    described: u32,
    pub(crate) resource: This,
    desc: [u32; 6],
}
const RTV_TAG: u32 = u32::from_le_bytes(*b"vRTV");
const _: () = assert!(size_of::<RtvRecord>() <= 112);

impl RtvRecord {
    /// All the view it records is made from: records with equal keys have
    /// equal views (of the same host texture) (`VICTORY_BIND_CACHE`, `ca1d519`).
    #[cfg(windows)]
    pub(crate) fn key(&self) -> (usize, [u32; 7]) {
        let mut words = [self.described; 7];
        words[1..].copy_from_slice(&self.desc);
        (self.resource as usize, words)
    }

    /// The view description in the host's type (the same layout).
    #[cfg(windows)]
    pub(crate) fn desc(
        &self,
    ) -> Option<windows::Win32::Graphics::Direct3D12::D3D12_RENDER_TARGET_VIEW_DESC> {
        const TEXTURE_2D: u32 = 4;
        self.host_desc(TEXTURE_2D)
    }

    /// The depth view description in the host's type (the same layout).
    #[cfg(windows)]
    pub(crate) fn depth_desc(
        &self,
    ) -> Option<windows::Win32::Graphics::Direct3D12::D3D12_DEPTH_STENCIL_VIEW_DESC> {
        const TEXTURE_2D: u32 = 3;
        self.host_desc(TEXTURE_2D)
    }

    /// The description words as a host type, the format as the host has it.
    /// Without one, a typeless host copy gets a 2D view (`dimension`) in
    /// the title's format, which the host needs spelled out.
    #[cfg(windows)]
    fn host_desc<T>(&self, dimension: u32) -> Option<T> {
        assert!(size_of::<T>() <= size_of::<[u32; 6]>());
        let mut words = if self.described != 0 {
            self.desc
        } else {
            [
                crate::resource::view_format(self.resource)?,
                dimension,
                0,
                0,
                0,
                0,
            ]
        };
        // A target or depth view: of a target, in its stand-in format.
        words[0] = crate::resource::target_format(crate::resource::pc_format(words[0]));
        // SAFETY: both host view descriptions are 24 bytes in this layout.
        Some(unsafe { (&raw const words).cast::<T>().read_unaligned() })
    }
}

/// Device method: records the view at `handle` (descriptor memory).
pub(crate) extern "system" fn create_rtv(
    _: This,
    resource: This,
    desc: *const [u32; 6],
    handle: u64,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: the title passes a description or null.
    let described = unsafe { desc.as_ref() };
    let record = RtvRecord {
        tag: RTV_TAG,
        described: u32::from(described.is_some()),
        resource,
        desc: described.copied().unwrap_or_default(),
    };
    // SAFETY: the handle is a descriptor slot, at least 112 bytes.
    unsafe { (handle as *mut RtvRecord).write_unaligned(record) };
}

/// Device method: a depth view, recorded as render target views are.
pub(crate) extern "system" fn create_dsv(
    this: This,
    resource: This,
    desc: *const [u32; 6],
    handle: u64,
) {
    create_rtv(this, resource, desc, handle);
    if handle != 0 {
        // SAFETY: just written by `create_rtv`.
        unsafe { (handle as *mut u32).write_unaligned(DSV_TAG) };
    }
}
const DSV_TAG: u32 = u32::from_le_bytes(*b"vDSV");

/// A shader view or sampler as the title described it (descriptions in the
/// host's layout, kept as words), for the host to build its own from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum View {
    /// Constant buffer: GPU address and size.
    Constant(u64, u32),
    /// Shader resource: resource, description (none: the resource's own).
    Shader(usize, Option<[u32; 10]>),
    /// Unordered access: resource, counter resource, description.
    Unordered(usize, usize, Option<[u32; 10]>),
    Sampler([u32; 13]),
    /// The console's own sampler description: flags, filters per axis,
    /// addressing, LOD and anisotropy settings, border color, then
    /// console-only tuning words.
    ConsoleSampler([u32; 27]),
    /// A view of memory without a resource object: whether it is unordered
    /// access, the resource description, then the view's (the plain one
    /// followed by the memory address and console fields).
    Placed(bool, [u32; 14], [u32; 16]),
}

impl View {
    /// Whether the view reads buffer memory (constant buffers, buffer views
    /// of resources or of memory) or memory the title placed a texture in:
    /// such views get host slots only for draws.
    #[cfg(windows)]
    fn is_buffer(&self) -> bool {
        match self {
            View::Constant(..) | View::Placed(..) => true,
            View::Shader(r, _) | View::Unordered(r, _, _) => {
                crate::resource::buffer_bytes(*r as This).is_some()
            }
            View::Sampler(_) | View::ConsoleSampler(_) => false,
        }
    }
}

/// Every distinct view in use; a descriptor holds a tag, an index into it
/// and the entry's generation, which survive titles copying descriptors as
/// bytes. A view of a resource lives as long as the resource: when the
/// title releases it, its entries are freed for reuse under a new
/// generation, so descriptors (and host slots made from them) naming the
/// old one go stale instead of naming whatever is made at the same address
/// next.
///
/// Views without a resource object have nothing that releases them, and a
/// title copies descriptors as bytes, so nothing says when the last
/// descriptor naming an entry is gone: such entries were kept for the run.
/// The title writes some 14,000 constant buffer views a frame,
/// each at the address its constants have that frame: 2,450,139 entries
/// after four minutes of play, 1,047 MB with the index, and 11,000 more
/// every second (run 65). So the two kinds that carry an address are not
/// entries any more: their records name them whole ([`constant_words`],
/// [`placed_words`]), and the table holds only what is bounded: views of
/// resource objects (freed with the resource), samplers (as many as there
/// are sampler states), and the shapes of placed views (their descriptions
/// without the address: as many as the title has kinds of textures and
/// buffers, however often it moves them).
/// `VICTORY_VIEW_INLINE=0`: every view is an entry, as before.
struct Views {
    /// Each entry's view and generation.
    list: Vec<(View, u32)>,
    index: std::collections::BTreeMap<View, u32>,
    /// Entries naming each resource.
    by_resource: std::collections::BTreeMap<usize, Vec<u32>>,
    free: Vec<u32>,
    /// The shapes of views of placed memory, and where each is.
    shapes: Vec<Shape>,
    shape_index: std::collections::BTreeMap<Shape, u32>,
}

/// Read far more often than written: every slot a draw reads through a
/// table is looked up here, on several recording threads at once in the
/// world. Readers share it; only a view not seen before (a new table entry
/// or shape) and a released resource take it alone.
static VIEWS: std::sync::RwLock<Views> = std::sync::RwLock::new(Views {
    list: Vec::new(),
    index: std::collections::BTreeMap::new(),
    by_resource: std::collections::BTreeMap::new(),
    free: Vec::new(),
    shapes: Vec::new(),
    shape_index: std::collections::BTreeMap::new(),
});
const VIEW_TAG: u32 = u32::from_le_bytes(*b"vVIE");

/// A record is the tag and two words, `i` and `generation` (the host keeps
/// them as one 64-bit key per slot). The top two bits of `generation` say
/// what the other 62 bits are: 0, entry `i` of the table at that
/// generation; [`CONSTANT`], a constant buffer view, whole; [`PLACED`], a
/// view of placed memory: its address and the number of its shape.
const KIND_SHIFT: u32 = 30;
/// Table generations stay below the kind bits (and above 0).
const GENERATIONS: u32 = (1 << KIND_SHIFT) - 1;
const CONSTANT: u32 = 1;
const PLACED: u32 = 2;
/// Bits of a constant buffer view's address in its record: any address a
/// program can have. The 15 bits above them are its size.
const CONSTANT_ADDRESS_BITS: u32 = 47;
/// Bits of a placed view's address in its record: placed memory is title
/// memory, which lies below 1 TiB. The 22 bits above them number its shape.
const PLACED_ADDRESS_BITS: u32 = 40;
const SHAPES: usize = 1 << (62 - PLACED_ADDRESS_BITS);

/// A view of placed memory without its address: whether it is unordered
/// access, the resource description, and the view's words but the two that
/// hold the address (10 and 11).
type Shape = (bool, [u32; 14], [u32; 14]);

/// Whether constant buffer views and views of placed memory are named whole
/// in their records ([`Views`]). On by default; `VICTORY_VIEW_INLINE=0`
/// makes every view a table entry, as before.
fn inline() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VIEW_INLINE").is_none_or(|v| v != "0"))
}

/// Views named whole in their records since the start: constant buffer
/// views, views of placed memory (for the memory line).
static NAMED_WHOLE: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// The two record words of the 62 bits `payload` of kind `kind`.
fn kind_words(kind: u32, payload: u64) -> [u32; 2] {
    [payload as u32, kind << KIND_SHIFT | (payload >> 32) as u32]
}

/// The 62 bits two record words carry beside their kind.
fn payload(i: u32, generation: u32) -> u64 {
    u64::from(generation & GENERATIONS) << 32 | u64::from(i)
}

/// The record words that name the constant buffer view of `size` bytes at
/// `address` whole; `None` if they cannot: an address past 47 bits, or a
/// size of 16 KiB or more that is no multiple of 256 or reaches 4 MiB (the
/// size has 15 bits: bytes below 16 KiB, else units of 256 bytes). Such a
/// view is a table entry.
fn constant_words(address: u64, size: u32) -> Option<[u32; 2]> {
    if address >> CONSTANT_ADDRESS_BITS != 0 {
        return None;
    }
    let code = if size < 1 << 14 {
        u64::from(size)
    } else if size.is_multiple_of(256) && size >> 8 < 1 << 14 {
        1 << 14 | u64::from(size >> 8)
    } else {
        return None;
    };
    Some(kind_words(CONSTANT, code << CONSTANT_ADDRESS_BITS | address))
}

/// The constant buffer view [`constant_words`] named.
fn constant_of(i: u32, generation: u32) -> View {
    let payload = payload(i, generation);
    let code = (payload >> CONSTANT_ADDRESS_BITS) as u32;
    let size = if code >> 14 == 0 { code } else { (code & 0x3fff) << 8 };
    View::Constant(payload & ((1 << CONSTANT_ADDRESS_BITS) - 1), size)
}

/// A placed view's shape and its address.
fn shape_of(unordered: bool, resource: &[u32; 14], desc: &[u32; 16]) -> (Shape, u64) {
    let mut rest = [0u32; 14];
    rest[..10].copy_from_slice(&desc[..10]);
    rest[10..].copy_from_slice(&desc[12..]);
    (
        (unordered, *resource, rest),
        u64::from(desc[10]) | u64::from(desc[11]) << 32,
    )
}

/// The record words that name the view of placed memory at `address` with
/// shape number `shape`.
fn placed_words(shape: u32, address: u64) -> [u32; 2] {
    kind_words(PLACED, u64::from(shape) << PLACED_ADDRESS_BITS | address)
}

impl Views {
    /// The number of `shape`, a new one if it is not there yet; `None` when
    /// the shapes are full (the view is then a table entry).
    fn shape(&mut self, shape: Shape) -> Option<u32> {
        if let Some(&at) = self.shape_index.get(&shape) {
            return Some(at);
        }
        if self.shapes.len() >= SHAPES {
            return None;
        }
        let at = self.shapes.len() as u32;
        self.shapes.push(shape);
        self.shape_index.insert(shape, at);
        Some(at)
    }

    /// The view of placed memory [`placed_words`] named.
    fn placed_of(&self, i: u32, generation: u32) -> Option<View> {
        let payload = payload(i, generation);
        let address = payload & ((1 << PLACED_ADDRESS_BITS) - 1);
        let &(unordered, resource, rest) = self.shapes.get((payload >> PLACED_ADDRESS_BITS) as usize)?;
        let mut desc = [0u32; 16];
        desc[..10].copy_from_slice(&rest[..10]);
        (desc[10], desc[11]) = (address as u32, (address >> 32) as u32);
        desc[12..].copy_from_slice(&rest[10..]);
        Some(View::Placed(unordered, resource, desc))
    }

    /// The record words of `view` as a table entry, if it has one.
    fn entry_of(&self, view: &View) -> Option<[u32; 2]> {
        let &i = self.index.get(view)?;
        Some([i, self.list[i as usize].1])
    }

    /// The record words of `view` as a table entry: its entry (a new or a
    /// freed one if the view is not there yet) and that entry's generation.
    fn entry(&mut self, view: View) -> [u32; 2] {
        let i = match self.index.get(&view) {
            Some(&i) => i,
            None => {
                let i = match self.free.pop() {
                    Some(i) => {
                        self.list[i as usize].0 = view.clone();
                        i
                    }
                    None => {
                        self.list.push((view.clone(), 1));
                        self.list.len() as u32 - 1
                    }
                };
                for r in view.resources().into_iter().filter(|&r| r != 0) {
                    self.by_resource.entry(r).or_default().push(i);
                }
                self.index.insert(view, i);
                i
            }
        };
        [i, self.list[i as usize].1]
    }
}

/// The two words that name `view` in a record: the view itself where its
/// kind allows ([`Views`]), else its table entry.
fn words_for(view: View) -> [u32; 2] {
    use std::sync::atomic::Ordering::Relaxed;
    if inline() {
        match &view {
            View::Constant(address, size) => {
                if let Some(words) = constant_words(*address, *size) {
                    NAMED_WHOLE[0].fetch_add(1, Relaxed);
                    return words;
                }
            }
            View::Placed(unordered, resource, desc) => {
                let (shape, address) = shape_of(*unordered, resource, desc);
                if address >> PLACED_ADDRESS_BITS == 0 {
                    // Nearly every placed view has a shape seen before.
                    let known = views_read().shape_index.get(&shape).copied();
                    let shape = known.or_else(|| views_write().shape(shape));
                    if let Some(shape) = shape {
                        NAMED_WHOLE[1].fetch_add(1, Relaxed);
                        return placed_words(shape, address);
                    }
                }
            }
            _ => {}
        }
    }
    // A view written before keeps its entry and generation.
    if let Some(words) = views_read().entry_of(&view) {
        return words;
    }
    views_write().entry(view)
}

fn views_read() -> std::sync::RwLockReadGuard<'static, Views> {
    VIEWS.read().unwrap_or_else(|e| e.into_inner())
}

fn views_write() -> std::sync::RwLockWriteGuard<'static, Views> {
    VIEWS.write().unwrap_or_else(|e| e.into_inner())
}

/// For the memory line: the distinct views kept ([`Views`]), those of them
/// free for reuse, and the bytes the table and its index hold for them (an
/// estimate: an entry of the table, and one of the index with its share of
/// a tree node; the shapes likewise); then the entries that are constant
/// buffer views and views of placed memory (those that did not fit a
/// record, or all of them with `VICTORY_VIEW_INLINE=0`), the shapes of
/// placed views kept, and the views named whole in their records since the
/// start: constant buffer views, views of placed memory.
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn views_kept() -> [u64; 8] {
    use std::sync::atomic::Ordering::Relaxed;
    let views = views_read();
    let each = size_of::<(View, u32)>() as u64;
    let shape = size_of::<Shape>() as u64;
    let (mut constants, mut placed) = (0, 0);
    for (view, _) in &views.list {
        match view {
            View::Constant(..) => constants += 1,
            View::Placed(..) => placed += 1,
            _ => {}
        }
    }
    [
        views.list.len() as u64,
        views.free.len() as u64,
        views.list.capacity() as u64 * each
            + views.index.len() as u64 * each * 3 / 2
            + views.shapes.capacity() as u64 * shape
            + views.shape_index.len() as u64 * shape * 3 / 2,
        constants,
        placed,
        views.shapes.len() as u64,
        NAMED_WHOLE[0].load(Relaxed),
        NAMED_WHOLE[1].load(Relaxed),
    ]
}

impl View {
    /// The resource objects the view names.
    fn resources(&self) -> [usize; 2] {
        match *self {
            View::Shader(r, _) => [r, 0],
            View::Unordered(r, counter, _) => [r, counter],
            _ => [0, 0],
        }
    }

    /// The texture object the view is of, if that texture has no host copy
    /// yet (`crate::resource::host_waits`): a slot written with such a
    /// view is left null until the texture is first used.
    #[cfg(windows)]
    fn waiting_texture(&self) -> Option<usize> {
        match *self {
            View::Shader(r, _) | View::Unordered(r, _, _)
                if crate::resource::host_waits(r as This) =>
            {
                Some(r)
            }
            _ => None,
        }
    }
}

/// Title slots (their handles) written with a view of a texture object
/// that had no host copy then, by texture. A title texture gets its host
/// copy on first use (`resource.rs` `Hosted`), and the title
/// writes each texture's views into its view heap when it creates the
/// texture: making those host slots would make every host copy at once.
/// They are made when the texture is: translated shaders index the heap
/// directly, so a slot is not always made again by the draw that reads it.
#[cfg(windows)]
static WAITING: std::sync::Mutex<std::collections::BTreeMap<usize, Vec<u64>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Textures given their host copy since [`make_waiting`] last ran.
#[cfg(windows)]
static MADE_LATE: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// Title texture `resource` was given its host copy (called under that
/// texture's own lock: nothing else is locked here).
#[cfg(windows)]
pub(crate) fn texture_made(resource: usize) {
    MADE_LATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(resource);
}

/// The slot at `handle` was written with a view of texture `resource`,
/// which has no host copy: it waits for one.
#[cfg(windows)]
fn wait_for(resource: usize, handle: u64) {
    /// A texture with more slots waiting than this is in use for all we
    /// know (its views are being copied about): its host copy is made.
    const MOST: usize = 64;
    let many = {
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        let slots = waiting.entry(resource).or_default();
        if !slots.contains(&handle) {
            slots.push(handle);
        }
        slots.len() > MOST
    };
    if many {
        // Makes it, and notes it made.
        let _ = crate::resource::host_copy(resource as This);
    } else if !crate::resource::host_waits(resource as This) {
        // Made by another thread since the slot was looked at: that making
        // may have been taken by `make_waiting` before this slot was noted.
        texture_made(resource);
    }
}

/// Makes the host slots that waited for a texture's host copy, for the
/// textures made since the last call. Called before a submission runs, by
/// the thread that submits: a slot is read when a list runs.
#[cfg(windows)]
pub(crate) fn make_waiting() {
    let made = std::mem::take(&mut *MADE_LATE.lock().unwrap_or_else(|e| e.into_inner()));
    for resource in made {
        let handles = WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&resource);
        for handle in handles.into_iter().flatten() {
            // Only if the slot still holds a view of that texture: the
            // title may have written it since (and a view of another
            // texture without a host copy must go on waiting).
            // (Looked up first: the handle is read only while its heap is
            // there.)
            let Some((heap, index)) = shader_slot(handle) else {
                continue;
            };
            let same = match view(handle) {
                Some(View::Shader(r, _) | View::Unordered(r, _, _)) => r == resource,
                _ => false,
            };
            if same && let Some(Some(host)) = heap.host.get() {
                host.mirror(index, handle, true);
            }
        }
    }
}

fn record(handle: u64, view: View) {
    if handle == 0 {
        return;
    }
    let [i, generation] = words_for(view);
    // SAFETY: the handle is a descriptor slot, at least 16 bytes.
    unsafe { (handle as *mut [u32; 3]).write_unaligned([VIEW_TAG, i, generation]) };
    #[cfg(windows)]
    mirror(handle);
}

/// The title released `resource`: the views naming it go stale.
pub(crate) fn forget(resource: usize) {
    #[cfg(windows)]
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&resource);
    let mut views = views_write();
    let Some(entries) = views.by_resource.remove(&resource) else {
        return;
    };
    for i in entries {
        let (view, generation) = views.list[i as usize].clone();
        // Entries freed through a view's other resource may hold another
        // view by now.
        if views.index.get(&view) != Some(&i) || !view.resources().contains(&resource) {
            continue;
        }
        views.index.remove(&view);
        // (Below the bits that say a record is no table entry.)
        views.list[i as usize].1 = ((generation + 1) & GENERATIONS).max(1);
        // The view's other resource (a counter) keeps a stale entry, which
        // `forget` of that resource skips.
        views.free.push(i);
    }
}

/// The view the record words `i` and `generation` name: the one they hold
/// whole, or the view of entry `i` if `generation` is still its own.
fn view_at(i: u32, generation: u32) -> Option<View> {
    match generation >> KIND_SHIFT {
        0 => {
            let views = views_read();
            let (view, current) = views.list.get(i as usize)?;
            (*current == generation).then(|| view.clone())
        }
        CONSTANT => Some(constant_of(i, generation)),
        PLACED => views_read().placed_of(i, generation),
        _ => None,
    }
}

/// The view recorded at `handle`, if one is (and its resource is alive).
pub(crate) fn view(handle: u64) -> Option<View> {
    if handle == 0 {
        return None;
    }
    // SAFETY: the handle is a descriptor slot the title passes back.
    let [tag, i, generation] = unsafe { (handle as *const [u32; 3]).read_unaligned() };
    if tag != VIEW_TAG {
        return None;
    }
    view_at(i, generation)
}

/// A description of `N` words, if the title passes one.
fn words<const N: usize>(desc: *const [u32; N]) -> Option<[u32; N]> {
    // SAFETY: the title passes a description or null.
    (!desc.is_null()).then(|| unsafe { desc.read_unaligned() })
}

/// Device method.
pub(crate) extern "system" fn create_cbv(_: This, desc: *const [u32; 4], handle: u64) {
    if let Some([lo, hi, size, _]) = words(desc) {
        let address = u64::from(lo) | u64::from(hi) << 32;
        record(handle, View::Constant(address, size));
    }
}

/// Device method.
pub(crate) extern "system" fn create_srv(
    _: This,
    resource: This,
    desc: *const [u32; 10],
    handle: u64,
) {
    record(handle, View::Shader(resource as usize, words(desc)));
}

/// Device method.
pub(crate) extern "system" fn create_uav(
    _: This,
    resource: This,
    counter: This,
    desc: *const [u32; 10],
    handle: u64,
) {
    let view = View::Unordered(resource as usize, counter as usize, words(desc));
    record(handle, view);
}

/// Device methods 0x230 and 0x238: shader resource and unordered access
/// views of memory the title placed itself, `(resource desc, view desc,
/// handle)`.
pub(crate) extern "system" fn create_placed_srv(
    _: This,
    resource: *const [u32; 14],
    desc: *const [u32; 16],
    handle: u64,
) {
    if let (Some(resource), Some(desc)) = (words(resource), words(desc)) {
        record(handle, View::Placed(false, resource, desc));
    }
}

pub(crate) extern "system" fn create_placed_uav(
    _: This,
    resource: *const [u32; 14],
    desc: *const [u32; 16],
    handle: u64,
) {
    if let (Some(resource), Some(desc)) = (words(resource), words(desc)) {
        record(handle, View::Placed(true, resource, desc));
    }
}

/// Device method.
pub(crate) extern "system" fn create_sampler(_: This, desc: *const [u32; 13], handle: u64) {
    if let Some(desc) = words(desc) {
        record(handle, View::Sampler(desc));
    }
}

/// Device method 0x1e8.
pub(crate) extern "system" fn create_sampler_x(_: This, desc: *const [u32; 27], handle: u64) {
    if let Some(desc) = words(desc) {
        record(handle, View::ConsoleSampler(desc));
    }
}

/// Device method: `count` descriptors of `kind` from `src` to `dst`.
pub(crate) extern "system" fn copy_simple(_: This, count: u32, dst: u64, src: u64, kind: u32) {
    let step = u64::from(increment(kind).unwrap_or(0));
    let len = u64::from(count) * step;
    if dst != 0 && src != 0 {
        // SAFETY: both ranges are descriptor slots the title owns.
        unsafe { std::ptr::copy(src as *const u8, dst as *mut u8, len as usize) };
        #[cfg(windows)]
        for i in 0..u64::from(count) {
            mirror(dst + i * step);
        }
    }
}

/// Device method: ranges of descriptors (start and count each) gathered
/// from the sources and scattered to the destinations, in order.
#[allow(clippy::too_many_arguments)] // the method's own arguments
pub(crate) extern "system" fn copy(
    _: This,
    dst_ranges: u32,
    dst_starts: *const u64,
    dst_sizes: *const u32,
    src_ranges: u32,
    src_starts: *const u64,
    src_sizes: *const u32,
    kind: u32,
) {
    let Some(step) = increment(kind) else {
        return;
    };
    // Each range as (start, count); a null size array means one descriptor
    // per range.
    let ranges = |n: u32, starts: *const u64, sizes: *const u32| -> Vec<(u64, u32)> {
        (0..n as usize)
            .map(|i| {
                // SAFETY: the title passes `n` starts and sizes (or null sizes).
                unsafe {
                    (
                        *starts.add(i),
                        if sizes.is_null() { 1 } else { *sizes.add(i) },
                    )
                }
            })
            .collect()
    };
    if dst_starts.is_null() || src_starts.is_null() {
        return;
    }
    let src = ranges(src_ranges, src_starts, src_sizes);
    let mut dst = ranges(dst_ranges, dst_starts, dst_sizes).into_iter();
    let (mut to, mut room) = dst.next().unwrap_or((0, 0));
    for (mut from, mut left) in src {
        while left > 0 {
            while room == 0 {
                let Some(next) = dst.next() else { return };
                (to, room) = next;
            }
            let n = left.min(room);
            copy_simple(std::ptr::null(), n, to, from, kind);
            let bytes = u64::from(n) * u64::from(step);
            (to, from, room, left) = (to + bytes, from + bytes, room - n, left - n);
        }
    }
}

/// The view recorded at `handle`, if one is.
pub(crate) fn rtv_record(handle: u64) -> Option<RtvRecord> {
    tagged_record(handle, RTV_TAG)
}

/// The depth view recorded at `handle`, if one is.
pub(crate) fn dsv_record(handle: u64) -> Option<RtvRecord> {
    tagged_record(handle, DSV_TAG)
}

fn tagged_record(handle: u64, tag: u32) -> Option<RtvRecord> {
    if handle == 0 {
        return None;
    }
    // SAFETY: the handle is a descriptor slot the title passes back.
    let record = unsafe { (handle as *const RtvRecord).read_unaligned() };
    (record.tag == tag).then_some(record)
}

/// Device method.
pub(crate) extern "system" fn create_heap(
    _: This,
    desc_ptr: *const HeapDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc_ptr.as_ref() }) else {
        return E_INVALIDARG;
    };
    let Some(size) = increment(desc.kind).map(|i| u64::from(i) * u64::from(desc.count.max(1)))
    else {
        return E_INVALIDARG;
    };
    // SAFETY: read only as a hint: a title that passes the plain
    // description has its own stack or data after it.
    let placement = unsafe { desc_ptr.add(1).cast::<Placement>().read_unaligned() };
    // shortcut: a placement is recognised by its size matching the count,
    // since the plain and extended descriptions share the slot and the
    // interface version; find the flag that tells them apart if a title
    // trips over this.
    let placed = placement.address != 0 && placement.size == size;
    tracing::debug!(?desc, placed, "descriptor heap");
    let (start, memory) = if placed {
        (placement.address, None)
    } else {
        match Memory::zeroed(size, 64) {
            Ok(memory) => (memory.address(), Some(memory)),
            Err(hr) => return hr,
        }
    };
    let visible = desc.flags & SHADER_VISIBLE != 0 && matches!(desc.kind, VIEWS_KIND | SAMPLERS);
    let heap = Box::new(Heap {
        object: Object::new(&FUNCTIONS),
        desc,
        start,
        memory,
        #[cfg(windows)]
        host: std::sync::OnceLock::new(),
        #[cfg(windows)]
        pending: Default::default(),
        #[cfg(windows)]
        written: (placed && visible)
            .then(|| crate::watch::track(start as usize, size as usize, true))
            .flatten(),
    });
    #[cfg(windows)]
    if visible {
        let at = std::ptr::from_ref::<Heap>(&heap) as usize;
        let mut heaps = SHADER_HEAPS.write().unwrap_or_else(|e| e.into_inner());
        heaps.push((start, start + size, at));
    }
    #[cfg(not(windows))]
    let _ = visible;
    object::hand_out(DESCRIPTOR_HEAP, heap, iid, &HEAP_IIDS, out)
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = object::is(iid, &HEAP_IIDS).then_some(this);
    object::answer(DESCRIPTOR_HEAP, iid, found, out)
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Heap>(this)
}
extern "system" fn desc(this: This, out: *mut HeapDesc) {
    if !out.is_null() {
        // SAFETY: checked non-null; the title passes room for a description.
        unsafe { out.write(heap(this).desc) };
    }
}
/// The same address for the CPU and the GPU start.
extern "system" fn start(this: This) -> u64 {
    heap(this).start
}

/// IUnknown 3, object 4, device child 1, heap 3.
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<DESCRIPTOR_HEAP, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    f[8] = desc as *const (); // 0x40
    f[9] = start as *const (); // 0x48 CPU
    f[10] = start as *const (); // 0x50 GPU
    Table(f)
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{S_OK, slot};

    #[test]
    fn sync_reads_stay_per_page_until_a_batch_is_asked_for() {
        let page = 64 << 10;
        let pages: Vec<_> = (0..4).map(|i| (i * page, (i + 1) * page)).collect();
        assert_eq!(plan_sync_reads(&pages, None, |_, _| false), pages);
        assert_eq!(
            plan_sync_reads(&pages, Some(page), |_, _| false),
            pages,
            "a batch of one watch page does not join neighbours"
        );
        assert_eq!(
            plan_sync_reads(&pages, Some(2 * page), |_, _| false),
            vec![(0, 2 * page), (2 * page, 4 * page)]
        );
        assert_eq!(
            plan_sync_reads(&pages, Some(1 << 20), |_, _| false),
            vec![(0, 4 * page)]
        );
    }

    #[test]
    fn sync_reads_do_not_cross_a_skipped_page() {
        let page = 64 << 10;
        let pages: Vec<_> = (0..4).map(|i| (i * page, (i + 1) * page)).collect();
        let kept = plan_sync_reads(&pages, Some(1 << 20), |from, _| from == page);
        assert_eq!(kept, vec![(0, page), (2 * page, 4 * page)]);
        assert!(plan_sync_reads(&pages, Some(1 << 20), |_, _| true).is_empty());
        assert_eq!(page_pieces(page / 2, page + 8, page), 2);
        assert_eq!(page_pieces(0, 0, page), 0);
    }

    /// What newer titles pass; tests use it throughout, so that reading the
    /// placement stays inside the value.
    #[repr(C)]
    struct Extended(HeapDesc, Placement);

    fn create(desc: HeapDesc, placement: Placement, out: &mut This) -> HRESULT {
        let extended = Extended(desc, placement);
        create_heap(std::ptr::null(), &extended.0, &HEAP_IIDS[0], out)
    }

    #[cfg(windows)]
    #[test]
    fn raw_heap_sync_stops_before_dynamic_host_slots() {
        let start = 0x1000usize;
        let step = 32;
        let capped = 5;
        assert_eq!(
            changed_slots(
                start + 3 * step + 4,
                start + 4 * step + 8,
                start,
                step as u64,
                capped
            ),
            3..5
        );
        assert_eq!(
            changed_slots(
                start + 6 * step,
                start + 9 * step,
                start,
                step as u64,
                capped
            ),
            5..5
        );
    }

    #[test]
    fn heaps_hold_count_descriptors_from_their_start() {
        let null = std::ptr::null();
        let desc = HeapDesc {
            kind: 0,
            count: 1000,
            flags: 1,
            nodes: 0,
        };
        let none = Placement {
            address: 0,
            size: 0,
        };
        let mut h = null;
        assert_eq!(create(desc, none, &mut h), S_OK);
        let cpu = slot::<extern "system" fn(This) -> u64>(h, 0x48)(h);
        assert_eq!(cpu % 64, 0);
        assert_eq!(slot::<extern "system" fn(This) -> u64>(h, 0x50)(h), cpu);
        assert_eq!(increment_size(null, 0), 32);
        // SAFETY: the heap holds 1000 descriptors of 32 bytes.
        unsafe { (cpu as *mut u8).add(999 * 32 + 31).write(1) };
        assert_eq!(increment_size(null, 9), 0);
        let bad = HeapDesc { kind: 9, ..desc };
        assert_eq!(create(bad, none, &mut h), E_INVALIDARG);
    }

    /// A descriptor copied into a placed heap as bytes is mirrored before
    /// the next submission, even though no descriptor method saw the copy.
    #[cfg(windows)]
    #[test]
    fn records_copied_as_bytes_are_made_at_the_next_sync() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        const MEM_WRITE_WATCH: u32 = 0x20_0000;
        let memory = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                crate::watch::PAGE,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        };
        assert!(!memory.is_null());
        let desc = HeapDesc {
            kind: SAMPLERS,
            count: 64,
            flags: SHADER_VISIBLE,
            nodes: 1,
        };
        let placement = Placement {
            address: memory as u64,
            size: 64 * 16,
        };
        let mut h: This = std::ptr::null();
        assert_eq!(create(desc, placement, &mut h), S_OK);
        if let Some(host) = heap(h).host() {
            let mut elsewhere = [0u64; 2];
            let source = elsewhere.as_mut_ptr() as u64;
            let sampler = [21, 3, 3, 3, 0, 1, 0, 0, 0, 0, 0, 0x7f7f_ffff, 0];
            create_sampler(std::ptr::null(), &sampler, source);
            // SAFETY: the heap slot and source both hold 16 descriptor bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    source as *const u8,
                    memory.cast::<u8>().add(5 * 16),
                    16,
                )
            };
            assert!(!host.made(5));
            sync_heaps();
            assert!(host.made(5));
        }
        slot::<extern "system" fn(This) -> u32>(h, 0x10)(h);
        // SAFETY: this allocation belongs to the test.
        unsafe { VirtualFree(memory, 0, MEM_RELEASE) };
    }

    /// Exercises the multi-threaded branch of `sync_heaps()` (`work.len() >= 16`)
    /// across 32 dirty pages of a placed CBV/SRV/UAV heap and verifies every
    /// written slot is mirrored without drops or clobbering untouched slots.
    #[cfg(windows)]
    #[test]
    fn parallel_sync_heaps_mirrors_many_dirty_pages() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        const MEM_WRITE_WATCH: u32 = 0x20_0000;
        const PAGES: usize = 32;
        const STEP: usize = 32;
        let bytes = PAGES * crate::watch::PAGE;
        let slots_per_page = crate::watch::PAGE / STEP;
        let count = (bytes / STEP) as u32;
        let memory = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                bytes,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        };
        assert!(!memory.is_null());
        let desc = HeapDesc {
            kind: 0, // CBV_SRV_UAV
            count,
            flags: SHADER_VISIBLE,
            nodes: 1,
        };
        let placement = Placement {
            address: memory as u64,
            size: bytes as u64,
        };
        let mut h: This = std::ptr::null();
        assert_eq!(create(desc, placement, &mut h), S_OK);
        if let Some(host) = heap(h).host() {
            let mut src = [0u64; 4];
            let src_at = src.as_mut_ptr() as u64;
            let mut written_slots = Vec::new();
            // Null SRVs (`View::Shader(0, None)`) are non-buffer views so
            // `sync_heaps()` (`for_draw = false`) mirrors them immediately.
            create_srv(std::ptr::null(), std::ptr::null(), std::ptr::null(), src_at);
            for p in 0..PAGES {
                let base = p * slots_per_page;
                for off in [0usize, 7, slots_per_page - 1] {
                    let idx = base + off;
                    // SAFETY: `idx * STEP + STEP <= bytes`.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            src_at as *const u8,
                            memory.cast::<u8>().add(idx * STEP),
                            STEP,
                        );
                    }
                    written_slots.push(idx as u32);
                }
            }
            for &idx in &written_slots {
                assert!(!host.made(idx), "slot {idx} should not be mirrored before sync_heaps");
            }
            sync_heaps();
            for &idx in &written_slots {
                assert!(host.made(idx), "slot {idx} was missed by parallel sync_heaps");
            }
            // Untouched neighbor slots must remain unmade.
            for p in 0..PAGES {
                let untouched = (p * slots_per_page + 3) as u32;
                assert!(!host.made(untouched), "untouched slot {untouched} unexpectedly mirrored");
            }
            // Second pass with no new writes must stay idempotent and clean.
            sync_heaps();
            // Third pass after mutating a subset of pages again with null UAVs.
            create_uav(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                src_at,
            );
            for p in (0..PAGES).step_by(2) {
                let idx = p * slots_per_page + 5;
                // SAFETY: `idx * STEP + STEP <= bytes`.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        src_at as *const u8,
                        memory.cast::<u8>().add(idx * STEP),
                        STEP,
                    );
                }
            }
            sync_heaps();
            for p in (0..PAGES).step_by(2) {
                let idx = (p * slots_per_page + 5) as u32;
                assert!(host.made(idx), "updated slot {idx} was missed on second parallel sync");
            }
        }
        slot::<extern "system" fn(This) -> u32>(h, 0x10)(h);
        // SAFETY: this allocation belongs to the test.
        unsafe { VirtualFree(memory, 0, MEM_RELEASE) };
    }

    #[test]
    fn views_survive_copies() {
        let null = std::ptr::null();
        let mut heap = vec![0u64; 4 * 8]; // four 32-byte slots
        let at = |i: usize| heap.as_ptr() as u64 + 32 * i as u64;
        let (s0, s1, s2, s3) = (at(0), at(1), at(2), at(3));
        create_cbv(null, &[0x1000, 1, 256, 0], s0);
        assert_eq!(view(s0), Some(View::Constant(0x1_0000_1000, 256)));
        create_srv(null, null, std::ptr::null(), s1);
        assert_eq!(view(s1), Some(View::Shader(0, None)));
        // The same view twice is one entry.
        create_srv(null, null, std::ptr::null(), s2);
        // SAFETY: both slots are in `heap`.
        assert_eq!(unsafe { *(s1 as *const [u32; 3]) }, unsafe {
            *(s2 as *const [u32; 3])
        });
        // Two ranges of one into one range of two.
        let (dst, dst_n, src, src_n) = ([s2], [2u32], [s0, s1], [1u32, 1]);
        copy(
            null,
            1,
            dst.as_ptr(),
            dst_n.as_ptr(),
            2,
            src.as_ptr(),
            src_n.as_ptr(),
            0,
        );
        assert_eq!(view(s2), view(s0));
        assert_eq!(view(s3), view(s1));
        assert_eq!(view(at(0) + 8), None);
        heap[0] = 0;
    }

    /// Constant buffer views are named whole in their records: any address
    /// a program can have, any size below 16 KiB, multiples of 256 bytes
    /// up to 4 MiB; what does not fit says so.
    #[test]
    fn constant_views_fit_their_records() {
        let top = (1u64 << CONSTANT_ADDRESS_BITS) - 1;
        for address in [0u64, 0x100, 0x2602_0000, 0xd_6b60_0000, 0x7ffd_a34f_0000, top] {
            for size in [0u32, 1, 255, 256, 4000, (1 << 14) - 1, 1 << 14, 65536, (1 << 22) - 256] {
                let [i, generation] = constant_words(address, size).expect("fits");
                assert_eq!(generation >> KIND_SHIFT, CONSTANT);
                assert_eq!(view_at(i, generation), Some(View::Constant(address, size)), "{address:#x} {size}");
            }
        }
        assert_eq!(constant_words(top + 1, 256), None, "an address past 47 bits");
        assert_eq!(constant_words(0x1000, (1 << 14) + 1), None, "16 KiB or more, no multiple of 256");
        assert_eq!(constant_words(0x1000, 1 << 22), None, "4 MiB");
        // Different views never share their words.
        assert_ne!(constant_words(0x1000, 256), constant_words(0x1000, 512));
        assert_ne!(constant_words(0x1000, 256), constant_words(0x1100, 256));
        assert_ne!(constant_words(0x1000, 1 << 14), constant_words(0x1000, 64));
    }

    /// The leak of run 65: a title that writes constant buffer views and
    /// views of placed memory at ever new addresses adds nothing to the
    /// table; one shape stands for all the placed views alike.
    #[test]
    fn views_at_new_addresses_add_no_entries() {
        if !inline() {
            return;
        }
        let null = std::ptr::null();
        let slots = [0u64; 4];
        let at = slots.as_ptr() as u64;
        // A resource description and a view no other test uses.
        let resource: [u32; 14] = std::array::from_fn(|k| 0x5ee0_0000 + k as u32);
        let mut desc: [u32; 16] = std::array::from_fn(|k| 0x7a70_0000 + k as u32);
        let mine = |v: &View| match v {
            View::Constant(address, _) => (0x6_0000_0000..0x6_1000_0000).contains(address),
            View::Placed(_, r, _) => *r == resource,
            _ => false,
        };
        let entries = || VIEWS.read().unwrap().list.iter().filter(|(v, _)| mine(v)).count();
        let shapes = || VIEWS.read().unwrap().shapes.iter().filter(|s| s.1 == resource).count();
        for n in 0..20_000u64 {
            let address = 0x6_0000_0000 + n * 0x100;
            create_cbv(null, &[address as u32, (address >> 32) as u32, 512, 0], at);
            assert_eq!(view(at), Some(View::Constant(address, 512)));
            let placed = 0xd_4940_0000 + n * 0x1_0000;
            (desc[10], desc[11]) = (placed as u32, (placed >> 32) as u32);
            create_placed_srv(null, &resource, &desc, at);
            assert_eq!(view(at), Some(View::Placed(false, resource, desc)));
        }
        assert_eq!(entries(), 0, "no table entries");
        assert_eq!(shapes(), 1, "one shape for 20,000 placed views");
        // The same shape, unordered: another shape.
        create_placed_uav(null, &resource, &desc, at);
        assert_eq!(view(at), Some(View::Placed(true, resource, desc)));
        assert_eq!(shapes(), 2);
        // Placed memory past 1 TiB (not title memory) is a table entry.
        (desc[10], desc[11]) = (0, 0x100);
        create_placed_srv(null, &resource, &desc, at);
        assert_eq!(view(at), Some(View::Placed(false, resource, desc)));
        assert_eq!(entries(), 1);
        // A record's words are the same for the same view (the host knows
        // a slot's view by them), and differ with the address.
        // SAFETY: both handles are slots of 32 bytes above.
        let words = |handle: u64| unsafe { *(handle as *const [u32; 3]) };
        let other = [0u64; 4];
        create_cbv(null, &[0x40, 6, 512, 0], at);
        create_cbv(null, &[0x40, 6, 512, 0], other.as_ptr() as u64);
        assert_eq!(words(at), words(other.as_ptr() as u64));
        create_cbv(null, &[0x140, 6, 512, 0], other.as_ptr() as u64);
        assert_ne!(words(at), words(other.as_ptr() as u64));
    }

    /// A released resource's views go stale; the same view of a new
    /// resource at the same address is a new record.
    #[test]
    fn views_die_with_their_resource() {
        let null = std::ptr::null();
        let slots = [0u64; 2 * 4];
        let (a, b) = (slots.as_ptr() as u64, slots.as_ptr() as u64 + 32);
        let resource = 0x7777_0000usize as This;
        create_srv(null, resource, std::ptr::null(), a);
        assert_eq!(view(a), Some(View::Shader(0x7777_0000, None)));
        forget(0x7777_0000);
        assert_eq!(view(a), None, "stale after release");
        create_srv(null, resource, std::ptr::null(), b);
        assert_eq!(view(b), Some(View::Shader(0x7777_0000, None)));
        // SAFETY: both slots are in `slots`.
        let (old, new) = unsafe { (*(a as *const [u32; 3]), *(b as *const [u32; 3])) };
        assert_ne!(old, new, "a new generation");
    }

    /// The title places its shader-visible heap in its own memory
    /// and checks that the heap starts there.
    #[test]
    fn placed_heaps_start_at_the_title_memory() {
        let mut memory = vec![0u8; 64 * 32];
        let address = memory.as_mut_ptr() as u64;
        let desc = HeapDesc {
            kind: 0,
            count: 64,
            flags: 1,
            nodes: 1,
        };
        let mut h = std::ptr::null();
        assert_eq!(
            create(
                desc,
                Placement {
                    address,
                    size: 64 * 32
                },
                &mut h
            ),
            S_OK
        );
        assert_eq!(slot::<extern "system" fn(This) -> u64>(h, 0x48)(h), address);
        assert_eq!(slot::<extern "system" fn(This) -> u64>(h, 0x50)(h), address);
        // A size that does not match the count is not a placement.
        assert_eq!(create(desc, Placement { address, size: 7 }, &mut h), S_OK);
        assert_ne!(slot::<extern "system" fn(This) -> u64>(h, 0x48)(h), address);
    }

    /// A fresh table built for a shader with `half_views` spans descriptors past
    /// the range starts given by `used` (`kitty-core/Kitty6` `6779c11`).
    #[cfg(windows)]
    #[test]
    fn a_fresh_table_holds_the_half_views_past_its_range_starts() {
        use victory_shader::{ResourceType, RootType, UsedResource};
        let Some(device) = crate::host::host() else {
            return;
        };
        let list = device.new_list().expect("a host list");
        let desc = HeapDesc {
            kind: VIEWS_KIND,
            count: 2,
            flags: SHADER_VISIBLE,
            nodes: 1,
        };
        let none = Placement {
            address: 0,
            size: 0,
        };
        let mut h: This = std::ptr::null();
        assert_eq!(create(desc, none, &mut h), S_OK);
        let title = heap(h);
        let host_heap = title.host().expect("a host view heap");
        let (first, second) = (title.handle(0).unwrap(), title.handle(1).unwrap());
        let memory = [0u64; 64];
        let address = memory.as_ptr() as u64;
        let resource = [1, 0, 0, 0, 512, 0, 1, 0x1_0001, 0, 1, 0, 1, 0, 0];
        let mut words = [0u32; 16];
        words[..10].copy_from_slice(&[10, 1, 5768, 0, 0, 0, 64, 0, 0, 0]);
        words[10] = address as u32;
        words[11] = (address >> 32) as u32;
        create_placed_srv(std::ptr::null(), &resource, &words, second);
        assert!(view(second).as_ref().and_then(host::half_view).is_some());
        let mut state = crate::draw::State::default();
        state.set_heaps(&[h]);
        state.set_table(true, 1, first);
        let range = UsedResource {
            kind: ResourceType::Srv,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 8,
            dwords: 4,
            user_data: 4,
            slot: 1,
        };
        let plan = crate::draw::TablePlan::new(&[range], &[], &[(1, 8)]);
        let made = state.tables(&list, &plan, true);
        let [(1, offset)] = made[..] else {
            panic!("one fresh table for root parameter 1, not {:?}", &made[..]);
        };
        assert_ne!(offset, u32::MAX, "the table found no dynamic slots");
        let next = host_heap.dynamic(1).expect("a dynamic slot");
        host_heap.release(host_heap.id(), next, 1);
        drop(state);
        slot::<extern "system" fn(This) -> u32>(h, 0x10)(h);
        assert_eq!(
            next,
            offset / 32 + 2,
            "the next allocation took the table's second slot"
        );
    }
}
