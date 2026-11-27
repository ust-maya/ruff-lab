# Victory Bench Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `victory bench` воспроизводимо доводит запуск до тюрьмы, считает 30 heavy-семплов `draws>=2500 && ms<=2000` и пишет JSON вне репо без ручного kill.

**Architecture:** Тонкая сабкоманда обёртка над существующим `run::run()` (без дублей маппинга) + минимальный счётчик в `Meter::frame` с deferred exit-потоком (save пайплайнов, tmp+rename JSON, `process::exit(0)`).

**Tech Stack:** Rust 2024 / 1.98.1 `x86_64-pc-windows-msvc`, `clap` 4, `anyhow`, `blake3` (уже зависимость), hand-rolled JSON без `serde_json`.

**Spec:** `docs/superpowers/specs/2026-11-28-bench-design.md`

## Global Constraints

- Windows-only toolchain `1.98.1`, `x86_64-pc-windows-msvc`; bench-код за `#[cfg(windows)]` рядом с `run.rs:7` / `run.rs:498` — не должен ломать Linux `cargo check`.
- CI: `cargo clippy --workspace --all-targets --locked -- -D warnings` + `--all-features`, `cargo test --workspace --locked`, `rustfmt max_width 100`.
- Дефолтный путь `run` не меняется; всё новое default off за `VICTORY_BENCH_HEAVY` / сабкомандой `Bench`.
- Asset policy `cargo xtask policy`: bench JSON только через `cfg.cache_subdir("bench")?` или `ensure_outside_repo(out)?` — никогда в `ruff-lab/`; НЕ писать fps в `README.md` / `logs/results.md` без кадра с RX 6650 XT.
- Bench JSON Schema `victory-bench/v1`, snake_case; `--frames` = число *qualifying heavy* семплов, не всего кадров (дефолт 30).
- Не заменять инкрементальный `heap_buffers` проход (`VICTORY_HEAP_RESCAN_FRAMES=64`) на полный скан.

## Review Focus

- `VICTORY_KEYS=1` без `:` не жмёт ничего: проверь что генератор всегда даёт `<sec>:<key>` списки и что порядок `Enter` раньше `Down` в `t=35`.
- Счётчик инкрементируется в 1-сек блоке, а не в 5-сек `logged`: проверь `N=30` достигает за ~30с heavy, а не 150с.
- Exit-per-frame удалит деструкторы/COM и порвёт JSON: проверь что exit только deferred-потоком после `present` + flush + tmp+rename.
- Грязные float-сериализации `35.400000000000006`: проверь целочисленную форматизацию `35.4`.
- `--out` внутрь репо обязан падать через `ensure_outside_repo`; проверь отказ с понятной ошибкой.

---

## File Structure

| File | Что это |
|------|---------|
| Create `apps/victory/src/bench.rs` | Обёртка: пресет env, генерал `VICTORY_KEYS`, резолв `--out`, вызов `run::run(..., SeriesS)`; все тесты генераторов |
| Modify `apps/victory/src/main.rs:6-12,30-124,138-202` | `mod bench;`, вариан `Command::Bench {exe, frames:u32=30, out:Option<PathBuf>, set:Vec<String>}`, диспатч `bench::run(&exe, frames, out, set)` |
| Modify `crates/victory-gpu/src/host.rs:7735-7744 (Meter), 7771-8006 (frame)` | `ON: OnceLock<Option<u64>>` + `parse_bench_target` + буфер `Vec<BenchSample>` в `Meter`, инкремент в 1-сек блоке, deferred spawn потока при достижении N |
| Create `crates/victory-gpu/src/bench.rs` (или `host.rs` подмодуль) | `BenchSample`, `bench_target()`, `is_heavy(draws,ms)`, `write_bench_json(path, samples)`, закрытый approx по строковым полям |
| Modify `README.md` + `docs/superpowers/specs/...` | Раздел с командами запуска микробенчей (без новых зависимостей) |
| Test `apps/victory/src/bench.rs` unit-тесты | Генератор ключей, `parse_set_arg`, `json_escape`, `median`, `is_heavy`, `parse_bench_target` |

## Tasks

### Task 1: CLI Bench skeleton

**Files:**
- Create: `apps/victory/src/bench.rs`
- Modify: `apps/victory/src/main.rs:6-12,30-124,138-202`
- Test: `apps/victory/src/bench.rs` (mod tests)

**Interfaces:**
- Consumes: `crate::{Console, Unimplemented}`, `run::run` сигнатура `apps/victory/src/run.rs:7-14`
- Produces: `pub fn run(exe:&Path, frames:u32, out:Option<PathBuf>, set:Vec<String>)->Result<()>` (Windows), стаб (не Windows)

- [ ] Step 1: Write test for `parse_set_arg` — `"A=1"` => `("A","1")`, `"NOEQ"` => Err, `"=x"` => Err
- [ ] Step 2: Run `cargo test -p victory bench::tests::parse_set_arg -- --exact --nocapture` — Expected: FAIL без bench.rs
- [ ] Step 3: Implement `mod bench;` в `main.rs:6-12` + `Command::Bench` `{exe:PathBuf, #[arg(long,default_value_t=30)] frames:u32, #[arg(long)] out:Option<PathBuf>, #[arg(long="set")] set:Vec<String>}` + диспатч + стаб `bench.rs` с обеими `#[cfg]` ветками, функцией `parse_set_arg(s:&str)->Result<(String,String)>`
- [ ] Step 4: Run `cargo test -p victory bench::tests` — Expected: PASS; `cargo clippy --workspace --all-targets --locked -- -D warnings` — Expected: PASS
- [ ] Step 5: Commit `git add apps/victory/src/bench.rs apps/victory/src/main.rs; git commit -m "feat: add victory bench cli skeleton"`

### Task 2: Preset env + VICTORY_KEYS generator

**Files:**
- Modify: `apps/victory/src/bench.rs`
- Test: `apps/victory/src/bench.rs` mod tests

**Interfaces:**
- Consumes: `wave9_victory_keys(end_s:u32)->String` продуцирует `VICTORY_KEYS`
- Produces: `fn apply_preset(set:&[String])->Result<()>`, `pub fn wave9_victory_keys(end_s:u32)->String`, `fn wave9_key_events(end_s:u32)->Vec<(f64,&'static str)>`

- [ ] Step 1: Write failing tests
  ```rust
  assert_eq!(wave9_victory_keys(3), "");
  assert!(wave9_victory_keys(40).contains("7:Enter,14:Enter"));
  // t=35: Enter перед Down, затем 35.4:Enter; нет 35.40000...; нет дублей
  ```
- [ ] Step 2: Run `cargo test -p victory bench::tests::wave9 -- --nocapture` — Expected: FAIL
- [ ] Step 3: Implement `wave9_victory_keys(1200)` целочисленно: `format!("{t}:Enter")`, `format!("{t}:Down")`, `format!("{t}.4:Enter")`; события отсортированы, деление 7/35 сохранено, `Enter` раньше `Down` при `t=35`
- [ ] Step 4: Implement `apply_preset`: ставит defaults если unset (таблица из §3 recon-2), `--set KEY=VAL` перекрывает, `VICTORY_BENCH_HEAVY=<frames>`, `VICTORY_BENCH_OUT=<путь>`; `unsafe std::env::set_var` с `// SAFETY:` комментарий до любого spawn_main (до `run.rs:126`)
- [ ] Step 5: Run `cargo test -p victory bench::tests` — Expected: PASS
- [ ] Step 6: Commit `git add apps/victory/src/bench.rs; git commit -m "feat: add bench preset and keys generator"`

### Task 3: Resolve out path + ensure pipelines

**Files:**
- Modify: `apps/victory/src/bench.rs`
- Test: `apps/victory/src/bench.rs` mod tests

**Interfaces:**
- Consumes: `Config::cache_subdir`, `ensure_outside_repo`
- Produces: `fn resolve_out(cfg:&Config, out:Option<PathBuf>, ts:String)->Result<PathBuf>`, `fn ensure_pipelines(cfg:&Config)->Result<()>`

- [ ] Step 1: Write failing test `resolve_out` — default => `cfg.cache_subdir("bench")/bench-<ts>.json`, явный `--out` внутри репо => Err
- [ ] Step 2: Run test — Expected: FAIL
- [ ] Step 3: Implement: `cfg.cache_subdir("bench")?` + `create_dir_all`, `ensure_outside_repo(out)?` для явного, `cfg.cache_subdir("pipelines/<SOURCE_HASH>")` существует иначе `bail!("run translate --cache --pipelines first")`, `out` прокинуть в `VICTORY_BENCH_OUT`
- [ ] Step 4: Run test — Expected: PASS
- [ ] Step 5: Commit `git add apps/victory/src/bench.rs; git commit -m "feat: resolve bench output path outside repo"`

### Task 4: GPU-side heavy counter + deferred exit

**Files:**
- Create: `crates/victory-gpu/src/bench.rs`
- Modify: `crates/victory-gpu/src/host.rs:7735-7744,7771-8006`, `crates/victory-gpu/src/lib.rs`
- Test: `crates/victory-gpu/src/bench.rs` mod tests

**Interfaces:**
- Consumes: `BenchSample {fps:f64, ms:f64, draws:f64, cpu:f64, gpu:f64, back_ms:f64, wall_ms:f64}`
- Produces: `pub fn bench_target()->Option<u64>`, `pub fn parse_bench_target(s:Option<&str>)->Option<u64>`, `pub fn is_heavy(draws:f64, ms:f64)->bool`, `pub fn write_bench_json(path:&Path, samples:&[BenchSample])->std::io::Result<()>`

- [ ] Step 1: Write failing unit tests: `parse_bench_target(None)=>None`, `parse_bench_target(Some("30"))=>Some(30)`, `parse_bench_target(Some("abc"))=>None`, `is_heavy(2500.0,2000.0)==true`, `is_heavy(2499.0,2000.0)==false`, `is_heavy(2500.0,2000.1)==false`
- [ ] Step 2: Run `cargo test -p victory-gpu bench -- --nocapture` — Expected: FAIL
- [ ] Step 3: Implement `bench.rs` + `static ON: OnceLock<Option<u64>>` в `host.rs`, расширить `Meter` полями `bench_started:Instant`, `bench_samples:Vec<BenchSample>`, `bench_done:bool`; инкремент в 1-сек блоке `host.rs:7771-7794` (НЕ в 5-сек `logged`) на уже вычисленных locals `fps/ms/draws/cpu/gpu/back_ms`
- [ ] Step 4: Implement deferred: при `count>=n` `AtomicBool` -> spawn thread -> `save_pipeline_cache()` -> tmp+rename JSON -> `std::process::exit(0)`; JSON writer через `tmp = path.with_extension("tmp")`, `fsync+rename`
- [ ] Step 5: Run `cargo test -p victory-gpu bench -- --nocapture` — Expected: PASS
- [ ] Step 6: Commit `git add crates/victory-gpu/src/bench.rs crates/victory-gpu/src/host.rs crates/victory-gpu/src/lib.rs; git commit -m "feat: count heavy bench samples in gpu frame line"`

### Task 5: Bench run wrapper + docs

**Files:**
- Modify: `apps/victory/src/bench.rs` (run impl)
- Modify: `README.md`, `docs/superpowers/specs/2026-11-28-bench-design.md`
- Test: manual + `cargo xtask policy`

**Interfaces:**
- Consumes: все выше
- Produces: `cargo clippy -D warnings` PASS + `cargo xtask policy` PASS

- [ ] Step 1: Implement `bench::run`: `apply_preset -> resolve_out -> ensure_pipelines -> run::run(&exe, Trap, false, true, SeriesS)` — вход/выход как в `run.rs:7-14`
- [ ] Step 2: Run `cargo clippy --workspace --all-targets --locked -- -D warnings` + `--all-features` + `cargo test --workspace --locked` — Expected: PASS все три
- [ ] Step 3: Run `cargo xtask policy` — Expected: PASS
- [ ] Step 4: Add docs: команды `cargo test --release -p victory-gpu --lib -- --ignored bench_mirror --nocapture`, `cargo test --release -p victory-shader bitcode -- --ignored --nocapture`, `cargo test --release -p victory-formats -- --ignored --nocapture` + секцию Bench usage в README
- [ ] Step 5: Commit `git add apps/victory/src/bench.rs README.md; git commit -m "docs: bench usage and microbench commands"`

## Spike: replay-захват кадра (throwaway, branch chore/replay-spike)

Успех = записать, вынести replay-бин, прокручивает 30 кадров за <30с в пределах 10% от live heavy median. Провал = спайк закрывается, остаётся Task 1-5 как финальный продукт. В прод не мержится автоматически.
