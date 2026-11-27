# Pipeline Overlap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Overlap next-frame CPU preparation with current GPU execution behind a flag default OFF.

**Architecture:** Keep incremental `heap_buffers` walk untouched; extract pure-CPU `prepare(runs) -> Prepared` from `Host::submit` and run it as `VICTORY_PREP_AHEAD=1` on a fixed `victory-prep` worker (requires `late()+relay()`), narrowing `submitted` lock to `Execute+Signal` only. Fallback to current `relay_wait`/`settle` on any overlap with `LINE.written`.

**Tech Stack:** Rust 1.98.1, Windows x86_64-pc-windows-msvc, D3D12, existing `LATE_WAIT`/`RELAY`/`WIDE_THREADS` patterns; no new dependencies.

**Spec:** Approach 1 from conversation (parallel pipeline). Full analysis lives in lead chat; this plan is the implementable slice.

## Global Constraints

- AGENTS.md: new speedup is flag default OFF until captured frame on RX 6650 XT + clean D3D12 Debug Layer run. Do not replace incremental `heap_buffers` walk (`VICTORY_HEAP_RESCAN_FRAMES` rotating slice) with full scan.
- CI must pass: `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked`, `rustfmt max_width=100`.
- Asset policy `cargo xtask policy` must pass; bench JSON only outside repo (`ensure_outside_repo`).
- FPS/ms numbers go to `logs/results.md` only if captured on reference host RX 6650 XT 8GB / Ryzen 5 8400F / 32GB / Win11 10.0.26300.

## Review Focus

- Descriptor heap ordering: background mirror must not race with a draw that reads same slot via table (`left_to_draws`).
- Lifetime: heap/texture staging buffers must outlive GPU flight they were prepared for (fence-gated retire, not immediate free).
- Submission lock: overload/backpressure — worker queue must bound (drop/defer, not grow unbounded when GPU stalls).
- Device-removed path: prep workers must observe removal and stop cleanly (see `removal_context`).
- Debug Layer: no new warnings with flag ON at 1280x720 / LOW_GRAPHICS=1.

---
### Task 1: Flag plumbing and bench preset knob
**Files:**
- Modify: `crates/victory-gpu/src/host.rs` (add `prep_ahead() -> bool` beside `late()`/`relay()` helpers, + unit test)
- Modify: `apps/victory/src/bench.rs` (document `--set VICTORY_PREP_AHEAD=1`, keep PRESET baseline unchanged)
**Interfaces:**
- Consumes: existing `OnceLock<bool>` flag pattern (`late()`, `relay()` in `host.rs`).
- Produces: `pub(crate) fn prep_ahead() -> bool` reads `VICTORY_PREP_AHEAD`, default false, effective only when `late() && relay()` else false.
- [ ] Write failing test for parsing default OFF and explicit `1` (effective requires late+relay).
- [ ] Run test to verify it fails.
- [ ] Implement `prep_ahead()`.
- [ ] Run tests to verify pass.
- [ ] Run `cargo clippy -p victory-gpu --locked -- -D warnings` to verify clean.
- [ ] Commit `feat(gpu): add VICTORY_PREP_AHEAD flag default off`.

### Task 2: Extract `prepare(runs) -> Prepared` and prep worker
**Files:**
- Modify: `crates/victory-gpu/src/host.rs` (`Host::submit` prepare block ~`#L5385-L5488` (`sync_heaps`, `make_waiting`, `run_deferred`, `take_snapshots`, `card_before`) moved to `prepare() -> Prepared` owning cloned Steps, Batch ranges, snapshots, CardPlan, writes/back keyed by `STARTED`)
- Create: prep worker `victory-prep` in same file or `crates/victory-gpu/src/prep.rs` (single fixed worker, bounded channel capacity = pipeline depth 2)
**Interfaces:**
- Consumes: `prep_ahead()` from Task 1, existing `sync_heaps` + stripe pattern, `burry()/FINISHED` retire discipline.
- Produces: `Prepared` struct + `PrepQueue { enqueue(PrepJob) -> JoinHandle-ish completion, wait_done(flight) }` whose `wait_done` falls back to `relay_wait` on `late_blocked` overlap (`Copy/Fill` overlapping `LINE.written`).
- [ ] Write failing test for bounded queue (enqueue > capacity blocks or defers, not panics).
- [ ] Run test to verify fail.
- [ ] Implement worker pool (names: `victory-prep` threads, respects `WIDE_THREADS`).
- [ ] Run tests to verify pass.
- [ ] Commit.

### Task 3: Overlap submit path (narrow lock under flag)
**Files:**
- Modify: `crates/victory-gpu/src/host.rs` (`Host::submit`: if `prep_ahead()` take `Prepared` from worker and hold `submitted` only around `ExecuteCommandLists+Signal`; else existing serial path, byte-identical)
**Interfaces:**
- Consumes: `prepare()`/`PrepQueue` from Task 2.
- Produces: flag-gated submit branch + `CARD_CLOCK_US` / `LATE_WAITED` evidence (`behind` shrinking).
- [ ] Add debug counters for overlapped prep hits (reuse `Meter` frame-line style).
- [ ] Run unit tests.
- [ ] Run `cargo test -p victory-gpu --lib -- --test-threads=1` to verify pass.
- [ ] Commit.

### Task 4: Verify and compare (bench A/B on target GPU)
**Files:**
- None (measurement only, JSON outside repo)
**Interfaces:**
- Consumes: `victory bench` harness.
- Produces: two comparable JSON reports (baseline vs `VICTORY_PREP_AHEAD=1`) + one `logs/results.md` row if captured on target GPU.
- [ ] Clean build `cargo build --release -p victory`.
- [ ] Warm-check `ensure_pipelines` (fail fast `run translate --cache --pipelines first` if cold).
- [ ] Run baseline `victory bench <exe> --frames 30 --out C:\bench\baseline.json`.
- [ ] Run variant `victory bench <exe> --frames 30 --out C:\bench\overlap.json --set VICTORY_PREP_AHEAD=1`.
- [ ] Compare heavy avg/med/peak fps + settled ms + relay wait; report delta. Only then consider default change; otherwise keep OFF.
- [ ] Run `cargo xtask policy` to verify no bench artifacts inside repo.
- [ ] Commit `docs(results): record pipeline overlap A/B` only if target-GPU run complete.
