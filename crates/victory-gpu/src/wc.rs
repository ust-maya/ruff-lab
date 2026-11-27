//! Write-combined title memory: stats and opt-in skip of copy-back
//! (`VICTORY_WC_NO_BACK=1`, off by default; Belobug #477).
//!
//! On the console the CPU writes such memory but does not read it back.
//! When the switch is on, declared shader writes that lie wholly in
//! write-combined title memory (at least `VICTORY_WC_NO_BACK_MIN_KB`,
//! default 64) and whose copy is carded and known write-watched stay on
//! the card: no snapshot, no compare, no copy-back. Narrower than full
//! VEH lazy copy-back (victory-fork #1178).
//!
//! `VICTORY_WC_STATS=1` (or `WC_NO_BACK`) tracks protection ranges and
//! per-kind upload/copy-back bytes. Full `PAGE_GUARD` census
//! (`VICTORY_WC_CENSUS`) is a follow-up; this port notes owned ranges only.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

/// Kinds of title memory, by what the title asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Kind {
    Unknown = 0,
    Cached = 1,
    WriteCombined = 2,
    Uncached = 3,
}

const KINDS: usize = 4;
const NAMES: [&str; KINDS] = ["unknown", "cached", "write-combined", "uncached"];

const UNCACHED: u32 = 0x200;
const WRITE_COMBINED: u32 = 0x400;

/// Whether title memory is sorted by kind: for the stats, or for [`no_back`].
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WC_STATS").is_some_and(|v| v == "1") || no_back())
}

/// `VICTORY_WC_NO_BACK=1` (off by default).
pub(crate) fn no_back() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WC_NO_BACK").is_some_and(|v| v == "1"))
}

fn no_back_min() -> usize {
    static MIN: OnceLock<usize> = OnceLock::new();
    *MIN.get_or_init(|| {
        std::env::var("VICTORY_WC_NO_BACK_MIN_KB")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(64)
            << 10
    })
}

/// Soft census of ranges left on the card (`VICTORY_WC_CENSUS=1`, needs
/// `WC_NO_BACK`). Notes size only; PAGE_GUARD arming is a follow-up.
pub(crate) fn census_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let asked = std::env::var_os("VICTORY_WC_CENSUS").is_some_and(|v| v == "1");
        if asked && !no_back() {
            tracing::warn!("VICTORY_WC_CENSUS needs VICTORY_WC_NO_BACK=1: off");
            return false;
        }
        if asked {
            tracing::warn!(
                "VICTORY_WC_CENSUS: noting ranges left on the card (soft census; PAGE_GUARD follow-up)"
            );
        }
        asked
    })
}

static SKIPPED: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static CENSUS_BYTES: AtomicU64 = AtomicU64::new(0);

/// Whether shaders' writes to `[address, address + len)` stay on the card.
pub(crate) fn skip_back(address: usize, len: usize, carded: bool, watched: bool) -> bool {
    if !no_back() || len == 0 {
        return false;
    }
    let wc = kind(address) == Kind::WriteCombined && kind(address + len - 1) == Kind::WriteCombined;
    if !wc {
        return false;
    }
    if len < no_back_min() {
        SKIPPED[2].fetch_add(1, Ordering::Relaxed);
        SKIPPED[3].fetch_add(len as u64, Ordering::Relaxed);
        return false;
    }
    if !carded || !watched {
        SKIPPED[if carded { 7 } else { 8 }].fetch_add(len as u64, Ordering::Relaxed);
        return false;
    }
    SKIPPED[0].fetch_add(1, Ordering::Relaxed);
    SKIPPED[1].fetch_add(len as u64, Ordering::Relaxed);
    true
}

/// Soft census: `bytes` of title memory left on the card.
pub(crate) fn census_note(bytes: u64) {
    if census_on() && bytes != 0 {
        CENSUS_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// A set of disjoint `[start, end)` ranges, joined where they touch.
#[derive(Default)]
pub(crate) struct Ranges(BTreeMap<usize, usize>);

impl Ranges {
    pub(crate) fn add(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (mut from, mut to) = (from, to);
        if let Some((&s, &e)) = self.0.range(..=from).next_back()
            && e >= from
        {
            if e >= to {
                return;
            }
            from = s;
        }
        let inside: Vec<usize> = self.0.range(from..=to).map(|(&s, _)| s).collect();
        for s in inside {
            if let Some(e) = self.0.remove(&s) {
                to = to.max(e);
            }
        }
        self.0.insert(from, to);
    }

    pub(crate) fn overlapping(&self, from: usize, to: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self
            .0
            .range(..from)
            .next_back()
            .filter(|(_, e)| **e > from)
            .map(|(&s, &e)| (s, e));
        first
            .into_iter()
            .chain(self.0.range(from..to).map(|(&s, &e)| (s, e)))
    }

    /// Soft-census / Drop follow-ups (Belobug #477); kept for the unit tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn remove(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let cut: Vec<(usize, usize)> = self.overlapping(from, to).collect();
        for (s, e) in cut {
            self.0.remove(&s);
            if s < from {
                self.0.insert(s, from);
            }
            if e > to {
                self.0.insert(to, e);
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn covers(&self, from: usize, to: usize) -> bool {
        from >= to || self.0.range(..=from).next_back().is_some_and(|(_, &e)| e >= to)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn contains(&self, at: usize) -> bool {
        self.0.range(..=at).next_back().is_some_and(|(_, &e)| e > at)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.0.iter().map(|(&s, &e)| (s, e))
    }
}

fn ranges() -> &'static RwLock<BTreeMap<usize, (usize, Kind)>> {
    static RANGES: OnceLock<RwLock<BTreeMap<usize, (usize, Kind)>>> = OnceLock::new();
    RANGES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn kind_of_protection(protect: u32) -> Kind {
    if protect & WRITE_COMBINED != 0 {
        Kind::WriteCombined
    } else if protect & UNCACHED != 0 {
        Kind::Uncached
    } else {
        Kind::Cached
    }
}

/// The title asked for `[at, at + len)` with the console's `protect`.
pub fn title_memory_protection(at: usize, len: usize, protect: u32) {
    if !on() || at == 0 || len == 0 {
        return;
    }
    insert(
        &mut ranges().write().unwrap_or_else(|e| e.into_inner()),
        at,
        at.saturating_add(len),
        kind_of_protection(protect),
    );
}

fn insert(map: &mut BTreeMap<usize, (usize, Kind)>, from: usize, to: usize, kind: Kind) {
    if let Some((&start, &(end, k))) = map.range(..from).next_back()
        && end > from
    {
        map.insert(start, (from, k));
        if end > to {
            map.insert(to, (end, k));
        }
    }
    let inside: Vec<usize> = map.range(from..to).map(|(&s, _)| s).collect();
    for start in inside {
        if let Some((end, k)) = map.remove(&start)
            && end > to
        {
            map.insert(to, (end, k));
        }
    }
    map.insert(from, (to, kind));
}

pub(crate) fn kind(address: usize) -> Kind {
    let map = ranges().read().unwrap_or_else(|e| e.into_inner());
    match map.range(..=address).next_back() {
        Some((_, &(end, k))) if address < end => k,
        _ => Kind::Unknown,
    }
}

static UP: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static BACK: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];

pub(crate) fn up(address: usize, bytes: u64) {
    if on() && bytes != 0 {
        UP[kind(address) as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

pub(crate) fn back(address: usize, bytes: u64) {
    if on() && bytes != 0 {
        BACK[kind(address) as usize].fetch_add(bytes, Ordering::Relaxed);
    }
}

pub(crate) type Sample = Option<([f64; KINDS], [f64; KINDS], [f64; 9])>;

pub(crate) fn take(frames: f64) -> Sample {
    if !on() {
        return None;
    }
    let take = |a: &[AtomicU64; KINDS]| -> [f64; KINDS] {
        std::array::from_fn(|i| a[i].swap(0, Ordering::Relaxed) as f64 / 1e6 / frames.max(1.0))
    };
    let per_frame = |v: u64| v as f64 / frames.max(1.0);
    let skipped = std::array::from_fn(|i| per_frame(SKIPPED[i].swap(0, Ordering::Relaxed)));
    Some((take(&UP), take(&BACK), skipped))
}

pub(crate) fn log(sample: Sample) {
    let Some((up, back, skipped)) = sample else {
        return;
    };
    let mut noted = [0u64; KINDS];
    for (start, (end, k)) in ranges().read().unwrap_or_else(|e| e.into_inner()).iter() {
        noted[*k as usize] += (end - start) as u64;
    }
    let part = |v: &[f64; KINDS]| {
        (0..KINDS)
            .map(|i| format!("{} {:.1}", NAMES[i], v[i]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    tracing::info!(
        "title memory by what the title asked for (VICTORY_WC_STATS), MB per frame: copied up {}; copied back {}; \
         noted so far: {}",
        part(&up),
        part(&back),
        (0..KINDS)
            .skip(1)
            .map(|i| format!("{} {} MB", NAMES[i], noted[i] >> 20))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if no_back() {
        let [n, bytes, small, small_bytes, _u, _ub, _uc, kept_unwatched, kept_uncarded] = skipped;
        tracing::info!(
            "shaders' writes into write-combined title memory left on the card (VICTORY_WC_NO_BACK, under {} KiB kept), \
             per frame: {n:.0} declarations, {:.0} MB declared; small ones kept: {small:.0}, {:.1} MB; \
             kept as before, not known watched: {:.1} MB, not carded: {:.1} MB",
            no_back_min() >> 10,
            bytes / 1e6,
            small_bytes / 1e6,
            kept_unwatched / 1e6,
            kept_uncarded / 1e6
        );
    }
    if census_on() {
        let noted = CENSUS_BYTES.swap(0, Ordering::Relaxed);
        tracing::info!(
            "VICTORY_WC_CENSUS (soft): {:.1} MB of ranges left on the card noted this second",
            noted as f64 / 1e6
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_protection_wins_and_cuts_older_ranges() {
        let mut map = BTreeMap::new();
        insert(&mut map, 0x1000, 0x9000, Kind::Cached);
        insert(&mut map, 0x3000, 0x5000, Kind::WriteCombined);
        let got: Vec<_> = map.iter().map(|(&s, &(e, k))| (s, e, k)).collect();
        assert_eq!(
            got,
            [
                (0x1000, 0x3000, Kind::Cached),
                (0x3000, 0x5000, Kind::WriteCombined),
                (0x5000, 0x9000, Kind::Cached)
            ]
        );
    }

    #[test]
    fn ranges_remove_cuts_and_cover() {
        let mut r = Ranges::default();
        r.add(0, 100);
        r.add(200, 300);
        r.remove(50, 250);
        assert_eq!(r.iter().collect::<Vec<_>>(), vec![(0, 50), (250, 300)]);
        assert!(r.covers(10, 50) && !r.covers(10, 51) && r.covers(260, 300));
        assert!(r.contains(0) && !r.contains(50) && r.contains(299) && !r.contains(300));
        r.remove(0, 1000);
        assert!(r.is_empty());
    }

    #[test]
    fn ranges_join_and_answer_overlaps() {
        let mut r = Ranges::default();
        r.add(10, 20);
        r.add(30, 40);
        r.add(20, 25);
        r.add(12, 15);
        assert_eq!(
            r.0.iter().map(|(&s, &e)| (s, e)).collect::<Vec<_>>(),
            [(10, 25), (30, 40)]
        );
        assert_eq!(r.overlapping(0, 11).collect::<Vec<_>>(), [(10, 25)]);
        assert_eq!(
            r.overlapping(25, 30).collect::<Vec<_>>(),
            Vec::<(usize, usize)>::new()
        );
        assert_eq!(
            r.overlapping(24, 31).collect::<Vec<_>>(),
            [(10, 25), (30, 40)]
        );
        r.add(5, 50);
        assert_eq!(
            r.0.iter().map(|(&s, &e)| (s, e)).collect::<Vec<_>>(),
            [(5, 50)]
        );
    }

    #[test]
    fn protection_bits_pick_the_kind() {
        assert_eq!(kind_of_protection(0x2404), Kind::WriteCombined);
        assert_eq!(kind_of_protection(0x2204), Kind::Uncached);
        assert_eq!(kind_of_protection(0x22004), Kind::Cached);
    }
}
