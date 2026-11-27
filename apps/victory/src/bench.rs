//! `victory bench`: reproducible benchmark wrapper over `run::run`.
//!
//! Applies the Wave 9 preset, resolves the bench JSON path outside the
//! repository, asserts the pipeline cache is warm, then launches the title
//! with `VICTORY_BENCH_HEAVY` set; the GPU side counts qualifying heavy
//! samples, writes the JSON and exits the process.

use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg_attr(not(windows), allow(dead_code))]
fn validate_frames(frames: u32) -> Result<()> {
    anyhow::ensure!(frames > 0, "--frames must be > 0");
    Ok(())
}

#[cfg(windows)]
pub fn run(exe: &Path, frames: u32, out: Option<PathBuf>, set: Vec<String>) -> Result<()> {
    validate_frames(frames)?;
    // Order per brief: apply_preset -> resolve_out -> ensure_pipelines ->
    // run::run. The resolved path is re-exported as VICTORY_BENCH_OUT
    // afterwards so the default (cache) location also reaches the GPU side,
    // which reads only that variable.
    apply_preset(&set, frames, out.as_deref())?;
    let cfg = victory_core::Config::load()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(|_| "0".to_owned(), |d| d.as_secs().to_string());
    let resolved = resolve_out(&cfg, out, ts)?;
    set_env("VICTORY_BENCH_OUT", &resolved.to_string_lossy());
    ensure_pipelines(&cfg)?;
    crate::run::run(
        exe,
        crate::Unimplemented::Trap,
        false,
        true,
        crate::Console::SeriesS,
    )
}

#[cfg(not(windows))]
pub fn run(_: &Path, _: u32, _: Option<PathBuf>, _: Vec<String>) -> Result<()> {
    anyhow::bail!("running titles requires Windows")
}

/// Parse one `--set KEY=VALUE` argument into its key and value.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_set_arg(s: &str) -> Result<(String, String)> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("expected KEY=VALUE, got {s:?}"))?;
    anyhow::ensure!(!key.is_empty(), "expected KEY=VALUE, got {s:?}");
    Ok((key.to_owned(), value.to_owned()))
}

/// Wave 9 shared preset from `docs/wave9-ab.md` ("Shared preset"), filled in
/// only when the variable is still unset. `VICTORY_SKIP_PIPELINES` is the
/// trimmed content of `docs/klbf/skip-amd.txt`. Deliberately NOT listed:
/// `VICTORY_IDLE_LISTS`, `VICTORY_LIST_CEILING`, `VICTORY_LIBRARY_STORE_LATER`
/// and the four Wave 7 `VICTORY_HEAP_*` flags — they stay unset unless the
/// user passes them via `--set`.
#[cfg_attr(not(windows), allow(dead_code))]
const PRESET: &[(&str, &str)] = &[
    ("VICTORY_FFX_SIMPLE", "1"),
    ("VICTORY_LOW_GRAPHICS", "1"),
    ("VICTORY_ONE_PAD", "1"),
    ("VICTORY_SHADER_OPT", "0"),
    ("VICTORY_SKIP_PIPELINES", "39f0e9aeb156a61c"),
    ("VICTORY_LATE_WAIT", "3"),
    ("VICTORY_RELAY_COPY_QUEUE", "0"),
    ("VICTORY_WIDE_BACK", "1"),
    ("VICTORY_WIDE_MERGE", "1"),
    ("VICTORY_WIDE_REFRESH", "1"),
    ("VICTORY_WIDE_THREADS", "6"),
    ("VICTORY_UNDER_WRITES_WORDS", "0"),
    ("VICTORY_UNDER_WRITES_RUNS", "100000"),
    ("VICTORY_UNDER_WRITES_MASKS", "1"),
    ("VICTORY_UNDER_WRITES_MASKS_LEAST", "1"),
    ("VICTORY_CARD_FETCH_INFLIGHT", "1"),
    ("VICTORY_COPY_CHANGED", "1"),
    ("VICTORY_RANGED", "1"),
    ("VICTORY_HEAP_RESCAN_FRAMES", "64"),
    ("VICTORY_CARD_BUFFERS_MB", "1024"),
    ("VICTORY_CARD_REHASH_RUNS", "1"),
    ("VICTORY_JOIN_COPIES", "1"),
    ("VICTORY_DSTORAGE_WORKERS", "3"),
    ("VICTORY_PIPELINE_WORKERS", "4"),
    ("VICTORY_PIPELINE_WAIT_MS", "250"),
    ("VICTORY_COMMIT_RESERVE_MB", "2048"),
    ("VICTORY_REMAP_LEAN", "1"),
    ("VICTORY_TEXTURE_MAX_DIM", "1024"),
    ("VICTORY_TEXTURE_LRU", "1"),
    ("VICTORY_TEXTURE_ROOM_PERCENT", "92"),
    ("VICTORY_TEXTURE_KEEP_PERCENT", "78"),
    ("VICTORY_DYNAMIC_SLOTS", "524288"),
    ("VICTORY_DISPLAY_MODE", "1280x720"),
    ("VICTORY_AMBIENT_SHADE", "0"),
];

/// Scripted input events for the Wave 9 A/B script (`docs/wave9-ab.md`):
/// `Enter` every 7 s, and at every 35 s a `Down` then an `Enter` 0.4 s later.
/// Times are seconds from process start, sorted ascending; at `t = 35` the
/// `Enter` (from the 7 s cadence) comes before `Down`.
#[allow(dead_code)]
fn wave9_key_events(end_s: u32) -> Vec<(f64, &'static str)> {
    let mut events = Vec::new();
    for t in 3..=end_s {
        if t % 7 == 0 {
            events.push((f64::from(t), "Enter"));
        }
        if t % 35 == 0 {
            events.push((f64::from(t), "Down"));
            events.push((f64::from(t) + 0.4, "Enter"));
        }
    }
    events
}

/// Renders [`wave9_key_events`] as the `<sec>:<key>,...` list consumed by
/// `VICTORY_KEYS` (read opaquely by `gameinput.rs` `scripted()`). Formatted
/// with integer math only: `t as f64 + 0.4` would render `35.400000...`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn wave9_victory_keys(end_s: u32) -> String {
    let mut parts = Vec::new();
    for t in 3..=end_s {
        if t % 7 == 0 {
            parts.push(format!("{t}:Enter"));
        }
        if t % 35 == 0 {
            parts.push(format!("{t}:Down"));
            parts.push(format!("{t}.4:Enter"));
        }
    }
    parts.join(",")
}

/// Sets one environment variable.
/// SAFETY: called single-threaded from `bench::run` before any game process
/// is spawned (before `run.rs` spawns the title), so no other thread can
/// observe a racing read/write of these variables.
#[cfg_attr(not(windows), allow(dead_code))]
fn set_env(key: &str, value: &str) {
    // SAFETY: see the doc comment above — no spawn and no concurrent access.
    unsafe { std::env::set_var(key, value) }
}

/// Applies the Wave 9 preset: fills [`PRESET`] defaults plus the generated
/// `VICTORY_KEYS` only where the variable is still unset, lets `--set
/// KEY=VALUE` override both the defaults and the environment, and always sets
/// `VICTORY_BENCH_HEAVY` from `frames` (and `VICTORY_BENCH_OUT` when an output
/// path is given). Must run before any spawn of the game process.
#[cfg_attr(not(windows), allow(dead_code))]
fn apply_preset(set: &[String], frames: u32, out: Option<&Path>) -> Result<()> {
    let mut overrides = Vec::new();
    for arg in set {
        overrides.push(parse_set_arg(arg)?);
    }
    let override_for = |key: &str| {
        overrides
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    for (key, value) in PRESET {
        if let Some(value) = override_for(key) {
            set_env(key, value);
        } else if std::env::var_os(key).is_none() {
            set_env(key, value);
        }
    }
    let keys;
    match override_for("VICTORY_KEYS") {
        Some(value) => set_env("VICTORY_KEYS", value),
        None if std::env::var_os("VICTORY_KEYS").is_none() => {
            keys = wave9_victory_keys(1200);
            set_env("VICTORY_KEYS", &keys);
        }
        None => {}
    }
    // Any other --set entry applies unconditionally.
    for (key, value) in &overrides {
        if !PRESET.iter().any(|(k, _)| k == key) && key != "VICTORY_KEYS" {
            set_env(key, value);
        }
    }
    set_env("VICTORY_BENCH_HEAVY", &frames.to_string());
    if let Some(out) = out {
        set_env("VICTORY_BENCH_OUT", &out.to_string_lossy());
    }
    Ok(())
}

/// Resolves the bench JSON output path.
///
/// Without an explicit `--out` this is `cfg.cache_subdir("bench")/bench-<ts>.json`
/// (created by `cache_subdir`). An explicit path must pass the asset policy
/// first — anything inside the repository is refused with a clear error.
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_out(cfg: &victory_core::Config, out: Option<PathBuf>, ts: String) -> Result<PathBuf> {
    if let Some(out) = out {
        victory_core::ensure_outside_repo(&out)?;
        Ok(out)
    } else {
        Ok(cfg.cache_subdir("bench")?.join(format!("bench-{ts}.json")))
    }
}

/// Asserts the RED pipeline cache is warm: the keyed
/// `pipelines/<SOURCE_HASH>/` directory under every `Config` cache must exist.
/// The check does not create the directory — a missing cache means the user
/// still has to run the prebuild first.
#[cfg_attr(not(windows), allow(dead_code))]
fn ensure_pipelines(cfg: &victory_core::Config) -> Result<()> {
    let dir = cfg
        .cache_dir
        .join("pipelines")
        .join(victory_shader::SOURCE_HASH);
    victory_core::ensure_outside_repo(&dir)?;
    if !dir.exists() {
        anyhow::bail!("run translate --cache --pipelines first");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_preset, ensure_pipelines, parse_set_arg, resolve_out, validate_frames,
        wave9_key_events, wave9_victory_keys,
    };
    use std::path::{Path, PathBuf};
    use victory_core::Config;

    #[test]
    fn validate_frames_rejects_zero() {
        assert!(validate_frames(0).is_err());
        assert!(validate_frames(30).is_ok());
    }

    #[test]
    fn parse_set_arg_parses_key_and_value() {
        assert_eq!(
            parse_set_arg("A=1").unwrap(),
            ("A".to_owned(), "1".to_owned())
        );
        assert!(parse_set_arg("NOEQ").is_err());
        assert!(parse_set_arg("=x").is_err());
    }

    #[test]
    fn wave9_keys_empty_before_first_enter() {
        assert_eq!(wave9_victory_keys(3), "");
        assert_eq!(wave9_victory_keys(6), "");
        assert_eq!(wave9_victory_keys(7), "7:Enter");
    }

    #[test]
    fn wave9_keys_golden_at_40() {
        // Brief: t=35 Enter before Down, then 35.4:Enter; no 35.40000... floats.
        assert_eq!(
            wave9_victory_keys(40),
            "7:Enter,14:Enter,21:Enter,28:Enter,35:Enter,35:Down,35.4:Enter"
        );
        assert!(wave9_victory_keys(40).contains("7:Enter,14:Enter"));
        assert!(!wave9_victory_keys(40).contains("35.40000"));
        assert!(!wave9_victory_keys(40).contains("35.400000"));
    }

    #[test]
    fn wave9_keys_sorted_and_integer_formatted() {
        let keys = wave9_victory_keys(1200);
        assert!(keys.starts_with("7:Enter,"));
        assert!(keys.contains("35:Enter,35:Down,35.4:Enter,42:Enter"));
        assert!(!keys.contains("35.40000"));
        assert!(!keys.contains("000000"));
        let events = wave9_key_events(40);
        for window in events.windows(2) {
            assert!(window[0].0 <= window[1].0, "events must be time-sorted");
        }
        assert_eq!(events[4], (35.0, "Enter"));
        assert_eq!(events[5], (35.0, "Down"));
        assert_eq!(events[6].1, "Enter");
        assert!((events[6].0 - 35.4).abs() < 1e-9);
    }

    fn cache_dir_in_temp() -> (Config, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "victory-bench-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = Config {
            cache_dir: base.clone(),
            ..Default::default()
        };
        (cfg, base)
    }

    #[test]
    fn resolve_out_default_uses_cache_bench_subdir() {
        let (cfg, base) = cache_dir_in_temp();
        let out = resolve_out(&cfg, None, "20261128-120000".to_owned()).unwrap();
        let expected = cfg
            .cache_subdir("bench")
            .unwrap()
            .join("bench-20261128-120000.json");
        assert_eq!(out, expected);
        assert!(expected.parent().unwrap().is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_out_explicit_inside_repo_is_refused() {
        let (cfg, base) = cache_dir_in_temp();
        let inside = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("bench-out.json");
        let err = resolve_out(&cfg, Some(inside), "x".to_owned()).unwrap_err();
        assert!(
            err.to_string().contains("inside") || err.to_string().contains("refusing"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_out_explicit_outside_repo_is_kept() {
        let (cfg, base) = cache_dir_in_temp();
        let out_path = base.join("custom").join("result.json");
        assert_eq!(
            resolve_out(&cfg, Some(out_path.clone()), "x".to_owned()).unwrap(),
            out_path
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_pipelines_requires_warm_cache() {
        let (cfg, base) = cache_dir_in_temp();
        let err = ensure_pipelines(&cfg).unwrap_err();
        assert_eq!(err.to_string(), "run translate --cache --pipelines first");
        std::fs::create_dir_all(base.join("pipelines").join(victory_shader::SOURCE_HASH)).unwrap();
        ensure_pipelines(&cfg).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_preset_sets_defaults_and_set_overrides() {
        // SAFETY: this is the only test touching the process environment and
        // it removes every variable it sets before returning.
        unsafe {
            for key in [
                "VICTORY_FFX_SIMPLE",
                "VICTORY_LOW_GRAPHICS",
                "VICTORY_ONE_PAD",
                "VICTORY_IDLE_LISTS",
                "VICTORY_LIST_CEILING",
                "VICTORY_HEAP_SKIP_TABLED",
                "VICTORY_BENCH_HEAVY",
                "VICTORY_BENCH_OUT",
                "VICTORY_KEYS",
            ] {
                std::env::remove_var(key);
            }
            std::env::set_var("VICTORY_ONE_PAD", "0");
            apply_preset(
                &["VICTORY_FFX_SIMPLE=0".to_owned(), "NOEQ".to_owned()],
                30,
                Some(&PathBuf::from(r"C:\bench\out.json")),
            )
            .unwrap_err();
            apply_preset(
                &["VICTORY_FFX_SIMPLE=0".to_owned()],
                30,
                Some(&PathBuf::from(r"C:\bench\out.json")),
            )
            .unwrap();
            assert_eq!(std::env::var("VICTORY_FFX_SIMPLE").unwrap(), "0");
            // Pre-existing user value wins over the default (except --set).
            assert_eq!(std::env::var("VICTORY_ONE_PAD").unwrap(), "0");
            // Defaults are filled in when unset.
            assert_eq!(std::env::var("VICTORY_LOW_GRAPHICS").unwrap(), "1");
            // Bench bookkeeping is always taken from the arguments.
            assert_eq!(std::env::var("VICTORY_BENCH_HEAVY").unwrap(), "30");
            assert_eq!(
                std::env::var("VICTORY_BENCH_OUT").unwrap(),
                r"C:\bench\out.json"
            );
            // VICTORY_KEYS is generated as "<sec>:<key>" entries.
            let keys = std::env::var("VICTORY_KEYS").unwrap();
            assert!(keys.starts_with("7:Enter,"));
            assert!(keys.contains("35:Enter,35:Down,35.4:Enter"));
            // These must stay unset: they regress the reference card (wave9-ab).
            assert!(std::env::var_os("VICTORY_IDLE_LISTS").is_none());
            assert!(std::env::var_os("VICTORY_LIST_CEILING").is_none());
            assert!(std::env::var_os("VICTORY_HEAP_SKIP_TABLED").is_none());
            for key in [
                "VICTORY_FFX_SIMPLE",
                "VICTORY_LOW_GRAPHICS",
                "VICTORY_ONE_PAD",
                "VICTORY_BENCH_HEAVY",
                "VICTORY_BENCH_OUT",
                "VICTORY_KEYS",
            ] {
                std::env::remove_var(key);
            }
        }
    }
}
