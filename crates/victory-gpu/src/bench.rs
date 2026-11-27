//! Bench-mode heavy-sample counting and deferred exit.
//!
//! The host frame line (`host.rs` `Meter::frame`) is the only place that
//! already computes `fps/ms/draws/cpu/gpu/back_ms` once a second from
//! consumed counters. Bench mode reads those locals only: it must never
//! re-consume the counters (`SUBMISSION_US`, `PREPARING`, `COPY_BACK`,
//! `take_*`), or the frame line would silently lose its samples.
//!
//! Counting happens in the 1-second block, not the 5-second `logged`
//! block: inside `logged` the counter would fire once per 5 seconds and
//! `N = 30` would take 150 s instead of ~30 s of heavy frames.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// One qualifying heavy 1-second sample from the host frame line.
///
/// `wall_ms` is measured from a never-reset clock (`Meter::bench_started`),
/// not from `since` (which resets every second for the `fps` aggregate).
#[derive(Debug, Clone, PartialEq)]
pub struct BenchSample {
    pub fps: f64,
    pub ms: f64,
    pub draws: f64,
    pub cpu: f64,
    pub gpu: f64,
    pub back_ms: f64,
    pub wall_ms: f64,
}

/// Zero-cost default: with `VICTORY_BENCH_HEAVY` unset (or unparseable) the
/// bench mode is off. Read once per process, so the frame line pays a single
/// branch per second, never per frame.
pub fn bench_target() -> Option<u64> {
    static TARGET: OnceLock<Option<u64>> = OnceLock::new();
    *TARGET.get_or_init(|| {
        let value = std::env::var("VICTORY_BENCH_HEAVY").ok();
        parse_bench_target(value.as_deref())
    })
}

/// Where the bench JSON goes (`VICTORY_BENCH_OUT`, set by the `victory
/// bench` preset). Read once per process alongside [`bench_target`]; the
/// host has no other way to reach the resolved output path, only
/// `VICTORY_BENCH_OUT` is plumbed through the environment.
pub fn bench_out_path() -> Option<PathBuf> {
    static OUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    OUT.get_or_init(|| std::env::var_os("VICTORY_BENCH_OUT").map(PathBuf::from))
        .clone()
}

/// Pure parse of `VICTORY_BENCH_HEAVY`: `None` (unset), empty, non-numeric
/// and zero values all mean off. Kept pure (no `OnceLock`, no environment)
/// so the unit tests below can exercise it directly.
pub fn parse_bench_target(s: Option<&str>) -> Option<u64> {
    s?.trim().parse::<u64>().ok().filter(|&n| n > 0)
}

/// A 1-second aggregate counts as heavy when it drew at least 2500 draws
/// per frame without blowing the 2000 ms/frame ceiling. Both bounds are
/// inclusive.
pub fn is_heavy(draws: f64, ms: f64) -> bool {
    draws >= 2500.0 && ms <= 2000.0
}

/// Claims the single deferred-exit slot. The frame line calls this only
/// once it has collected `N` heavy samples; the first caller wins and every
/// later caller sees `false`, so the exit thread is spawned at most once.
pub fn try_begin_exit() -> bool {
    static EXITING: AtomicBool = AtomicBool::new(false);
    !EXITING.swap(true, Ordering::SeqCst)
}

/// Writes the bench JSON atomically: bytes go to a temporary file beside
/// `path`, are flushed to disk, and are then renamed over `path`, so a
/// reader never sees a half-written file (same scheme as the pipeline
/// library's `write_atomically`).
pub fn write_bench_json(path: &Path, samples: &[BenchSample]) -> std::io::Result<()> {
    let mut text = String::new();
    text.push_str("{\"schema\":\"victory-bench/v1\",\"heavy_n\":");
    text.push_str(&samples.len().to_string());
    text.push_str(",\"samples\":[");
    for (i, sample) in samples.iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str(&format!(
            "{{\"fps\":{},\"ms\":{},\"draws\":{},\"cpu\":{},\"gpu\":{},\"back_ms\":{},\"wall_ms\":{}}}",
            fmt_f64(sample.fps),
            fmt_f64(sample.ms),
            fmt_f64(sample.draws),
            fmt_f64(sample.cpu),
            fmt_f64(sample.gpu),
            fmt_f64(sample.back_ms),
            fmt_f64(sample.wall_ms),
        ));
    }
    text.push_str("]}\n");
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The shortest round-trippable rendering of a finite float: `35.4` stays
/// `35.4` instead of the `35.400000...` a `t + 0.4` computation would give.
fn fmt_f64(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "null".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_heavy, parse_bench_target, write_bench_json, BenchSample};
    use std::path::PathBuf;

    #[test]
    fn parse_bench_target_unset_is_none() {
        assert_eq!(parse_bench_target(None), None);
    }

    #[test]
    fn parse_bench_target_parses_count() {
        assert_eq!(parse_bench_target(Some("30")), Some(30));
    }

    #[test]
    fn parse_bench_target_garbage_is_none() {
        assert_eq!(parse_bench_target(Some("abc")), None);
    }

    #[test]
    fn is_heavy_boundary_is_inclusive() {
        assert!(is_heavy(2500.0, 2000.0));
    }

    #[test]
    fn is_heavy_draws_just_below_threshold() {
        assert!(!is_heavy(2499.0, 2000.0));
    }

    #[test]
    fn is_heavy_ms_just_above_ceiling() {
        assert!(!is_heavy(2500.0, 2000.1));
    }

    #[test]
    fn write_bench_json_round_trips_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("bench.json");
        let samples = vec![BenchSample {
            fps: 35.4,
            ms: 28.2,
            draws: 2500.0,
            cpu: 10.0,
            gpu: 12.0,
            back_ms: 1.5,
            wall_ms: 30000.0,
        }];
        write_bench_json(&path, &samples).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"schema\":\"victory-bench/v1\""));
        assert!(text.contains("\"heavy_n\":1"));
        assert!(text.contains("\"fps\":35.4"));
        assert!(!text.contains("35.40000"));
        assert!(!text.contains("35.400000"));
    }
}
