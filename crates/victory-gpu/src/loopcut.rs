//! Decisions for the RAM↔VRAM loop: which 4 KiB pages are dirty, how they
//! pack into an upload ring, when a direct queue must wait for a copy
//! queue, and the frame line those choices are judged by.
//!
//! The GPU calls stay in `host` and `shadow`. This module is only the
//! numbers and the pure rules, so they can be checked without a device.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Bytes of one upload ring. A heavy frame's dirty pages are a few
/// megabytes; two rings cover one frame of overlap.
pub(crate) const RING_BYTES: u64 = 32 << 20;
/// Copy offsets in the ring are multiples of this.
pub(crate) const RING_ALIGN: u64 = 256;

const N: usize = 12;
const COPY_BYTES: usize = 0;
const DESCRIPTOR_US: usize = 1;
const PREP_US: usize = 2;
const SUBMIT_WAIT_US: usize = 3;
const GUARD_FAULTS: usize = 4;
const WATCHED_PAGES: usize = 5;
const L0_FALLBACKS: usize = 6;
const RING_BYTES_N: usize = 7;
const IMMUTABLE_SKIPS: usize = 8;
const DS_BYTES: usize = 9;
const COPY_SIGNALS: usize = 10;
const WATCHDOGS: usize = 11;

static COUNT: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

fn add(i: usize, n: u64) {
    if n != 0 {
        COUNT[i].fetch_add(n, Relaxed);
    }
}

pub(crate) fn note_copy(bytes: u64) {
    add(COPY_BYTES, bytes);
}
pub(crate) fn note_descriptor(us: u64) {
    add(DESCRIPTOR_US, us);
}
pub(crate) fn note_prep(us: u64) {
    add(PREP_US, us);
}
pub(crate) fn note_submit_wait(us: u64) {
    add(SUBMIT_WAIT_US, us);
}
pub(crate) fn note_fault() {
    add(GUARD_FAULTS, 1);
}
pub(crate) fn note_watched(pages: u64) {
    add(WATCHED_PAGES, pages);
}
pub(crate) fn note_l0() {
    add(L0_FALLBACKS, 1);
}
pub(crate) fn note_ring(bytes: u64) {
    add(RING_BYTES_N, bytes);
}
pub(crate) fn note_immutable(bytes: u64) {
    add(IMMUTABLE_SKIPS, 1);
    let _ = bytes;
}
pub(crate) fn note_stream(bytes: u64) {
    add(DS_BYTES, bytes);
}
pub(crate) fn note_copy_signal() {
    add(COPY_SIGNALS, 1);
}
pub(crate) fn note_watchdog() {
    add(WATCHDOGS, 1);
}

static LAST_LIST: Mutex<String> = Mutex::new(String::new());

pub(crate) fn note_list(text: impl Into<String>) {
    *LAST_LIST.lock().unwrap_or_else(|e| e.into_inner()) = text.into();
}

pub(crate) fn last_list() -> String {
    LAST_LIST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Page-granular dirty uploads, on unless `VICTORY_DIRTY_RING=0`.
pub(crate) fn dirty_ring() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_DIRTY_RING").is_none_or(|v| v != "0"))
}

/// `VICTORY_REBAR=1` asks for the write-combined L1 ring. The probe has to
/// have succeeded for at least `ring` bytes or the upload heap is used.
pub(crate) fn use_rebar(ring: u64) -> bool {
    rebar_flag() && REBAR_L1.load(Relaxed) && REBAR_BYTES.load(Relaxed) >= ring
}

fn rebar_flag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_REBAR").is_some_and(|v| v == "1"))
}

static REBAR_L1: AtomicBool = AtomicBool::new(false);
static REBAR_BYTES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn set_rebar_probe(l1: bool, bytes: u64) {
    REBAR_L1.store(l1, Relaxed);
    REBAR_BYTES.store(bytes, Relaxed);
}

pub(crate) fn rebar_probe() -> (bool, u64) {
    (REBAR_L1.load(Relaxed), REBAR_BYTES.load(Relaxed))
}

/// How long a direct-fence value may stay unfinished before the watchdog
/// logs. `VICTORY_FENCE_TIMEOUT_MS`, default 8 s; `0` logs nothing.
pub(crate) fn fence_timeout_us() -> u64 {
    static US: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *US.get_or_init(|| {
        std::env::var("VICTORY_FENCE_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(8_000)
            .saturating_mul(1_000)
    })
}

/// The frame line. Counters reset, so one caller per frame prints it.
pub(crate) fn take_line(frames: f64) -> String {
    let d = if frames > 0.0 { frames } else { 1.0 };
    let c: [f64; N] = std::array::from_fn(|i| COUNT[i].swap(0, Relaxed) as f64 / d);
    let watched = c[WATCHED_PAGES];
    let faults = c[GUARD_FAULTS];
    let share = if watched + faults > 0.0 {
        100.0 * faults / (watched + faults)
    } else {
        0.0
    };
    let (l1, bytes) = rebar_probe();
    format!(
        "memory loop per frame: copy {:.1} MB; descriptors {:.1} ms; prep {:.1} ms; submit wait {:.1} ms; \
         guard faults {:.2}% of watched pages ({:.0} faults, {:.0} pages); VRAM refused → L0 {:.0}; \
         ring {:.1} MB; immutable skips {:.0}; DirectStorage to GPU {:.1} MB; copy signals {:.0}; \
         fence timeouts {:.0}; rebar probe l1={l1} {} MB tier {}",
        c[COPY_BYTES] / 1e6,
        c[DESCRIPTOR_US] / 1e3,
        c[PREP_US] / 1e3,
        c[SUBMIT_WAIT_US] / 1e3,
        share,
        faults,
        watched,
        c[L0_FALLBACKS],
        c[RING_BYTES_N] / 1e6,
        c[IMMUTABLE_SKIPS],
        c[DS_BYTES] / 1e6,
        c[COPY_SIGNALS],
        c[WATCHDOGS],
        bytes >> 20,
        if use_rebar(RING_BYTES) { "on" } else { "off" }
    )
}

/// 4 KiB pages (aligned addresses) that fall in `[start, end)`, joined
/// where they touch, clipped to the range. Empty when none do.
pub(crate) fn coalesce_pages(start: usize, end: usize, pages: &[usize]) -> Vec<(usize, usize)> {
    let mut ps: Vec<usize> = pages
        .iter()
        .copied()
        .filter(|&p| p < end && p.saturating_add(4096) > start)
        .collect();
    ps.sort_unstable();
    ps.dedup();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for p in ps {
        let a = p.max(start);
        let b = p.saturating_add(4096).min(end);
        if a >= b {
            continue;
        }
        if let Some(last) = out.last_mut()
            && last.1 == a
        {
            last.1 = b;
        } else {
            out.push((a, b));
        }
    }
    out
}

/// Where `len` bytes go in a ring of `capacity` that is filled to `cursor`.
/// `None` when they do not fit. The next cursor is past the bytes.
pub(crate) fn pack_offset(cursor: u64, capacity: u64, len: u64, align: u64) -> Option<(u64, u64)> {
    if len == 0 || align == 0 || len > capacity {
        return None;
    }
    let src = cursor.checked_add(align - 1)? & !(align - 1);
    let next = src.checked_add(len)?;
    (next <= capacity).then_some((src, next))
}

/// A ring whose last reader has finished (`ticket <= completed`).
pub(crate) fn pick_ring(completed: u64, tickets: &[u64]) -> Option<usize> {
    tickets.iter().position(|&ticket| ticket <= completed)
}

/// The direct queue waits for this copy-fence value when any copy was
/// signalled. `None` when nothing was.
pub(crate) fn copy_wait(signalled: u64) -> Option<u64> {
    (signalled > 0).then_some(signalled)
}

/// A direct fence armed at `at_us` is late at `now_us` when `limit_us` is set.
pub(crate) fn watchdog_due(now_us: u64, at_us: u64, limit_us: u64) -> bool {
    limit_us > 0 && now_us.saturating_sub(at_us) >= limit_us
}

/// Hot `heap_buffers` passes use only slots the descriptor heap marked
/// dirty. A full walk is the background pass every `every` frames.
pub(crate) fn rescan_this_pass(pass: u64, every: u64) -> bool {
    pass > 0 && every > 0 && pass.is_multiple_of(every)
}

static COVERS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static INFLIGHT: Mutex<Vec<(u64, usize, usize)>> = Mutex::new(Vec::new());

/// DirectStorage finished writing title memory `[from, to)`. A later refresh
/// of that range does not copy it again.
pub(crate) fn cover_stream(from: usize, to: usize) {
    if from >= to {
        return;
    }
    let mut c = COVERS.lock().unwrap_or_else(|e| e.into_inner());
    match c.last_mut() {
        Some(last) if from <= last.1 && to >= last.0 => *last = (last.0.min(from), last.1.max(to)),
        _ => c.push((from, to)),
    }
}

pub(crate) fn covers() -> Vec<(usize, usize)> {
    COVERS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Parts of `[from, to)` that no cover holds.
pub(crate) fn subtract_covered(
    from: usize,
    to: usize,
    covers: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if from >= to {
        return Vec::new();
    }
    let mut gaps = vec![(from, to)];
    for &(c0, c1) in covers {
        let mut next = Vec::new();
        for (a, b) in gaps {
            if c1 <= a || c0 >= b {
                next.push((a, b));
                continue;
            }
            if a < c0 {
                next.push((a, c0.min(b)));
            }
            if c1 < b {
                next.push((c1.max(a), b));
            }
        }
        gaps = next;
    }
    gaps.retain(|(a, b)| a < b);
    gaps
}

pub(crate) fn note_inflight(id: u64, from: usize, to: usize) {
    if from >= to {
        return;
    }
    INFLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, from, to));
}

pub(crate) fn inflight_covers(id: u64, from: usize, to: usize) -> bool {
    INFLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|&(i, a, b)| i == id && a <= from && to <= b)
}

/// Covers and in-flight notes belong to the submission that consumed them.
pub(crate) fn end_submit() {
    COVERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    INFLIGHT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::{
        coalesce_pages, copy_wait, pack_offset, pick_ring, rescan_this_pass, subtract_covered,
        watchdog_due,
    };

    #[test]
    fn coalesce_joins_dirty_pages_and_skips_gaps() {
        let pages = [0x2000, 0x0000, 0x1000, 0x4000];
        assert_eq!(
            coalesce_pages(0, 0x5000, &pages),
            vec![(0, 0x3000), (0x4000, 0x5000)]
        );
        assert_eq!(
            coalesce_pages(0x1800, 0x2800, &[0x1000, 0x2000]),
            vec![(0x1800, 0x2800)]
        );
        assert!(coalesce_pages(0, 0x1000, &[0x5000]).is_empty());
    }

    #[test]
    fn pack_offset_aligns_and_rejects_overflow() {
        assert_eq!(pack_offset(1, 100, 16, 8), Some((8, 24)));
        assert_eq!(pack_offset(90, 100, 16, 8), None);
        assert_eq!(pack_offset(0, 32, 0, 8), None);
        assert_eq!(pack_offset(0, 16, 32, 8), None);
    }

    #[test]
    fn pick_ring_is_a_finished_ticket() {
        assert_eq!(pick_ring(4, &[5, 4, 9]), Some(1));
        assert_eq!(pick_ring(3, &[5, 4]), None);
        assert_eq!(pick_ring(0, &[0]), Some(0));
    }

    #[test]
    fn copy_wait_and_watchdog() {
        assert_eq!(copy_wait(0), None);
        assert_eq!(copy_wait(3), Some(3));
        assert!(!watchdog_due(1_000, 0, 0));
        assert!(!watchdog_due(999, 0, 1_000));
        assert!(watchdog_due(1_000, 0, 1_000));
        assert!(!watchdog_due(5, 10, 1_000));
    }

    #[test]
    fn rescan_is_not_every_hot_frame() {
        assert!(!rescan_this_pass(0, 64));
        assert!(!rescan_this_pass(1, 64));
        assert!(!rescan_this_pass(63, 64));
        assert!(rescan_this_pass(64, 64));
        assert!(!rescan_this_pass(64, 0));
    }

    #[test]
    fn subtract_covered_drops_streamed_bytes_only() {
        let covers = [(100, 140), (180, 200)];
        assert_eq!(
            subtract_covered(80, 220, &covers),
            vec![(80, 100), (140, 180), (200, 220)]
        );
        assert!(subtract_covered(100, 140, &covers).is_empty());
        assert_eq!(subtract_covered(0, 10, &covers), vec![(0, 10)]);
    }
}
