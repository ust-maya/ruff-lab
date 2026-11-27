# AGENTS.md

Instructions for coding agents. Build and run steps are in `README.md`. The asset rule is in `docs/asset-policy.md`. This file only covers what those two do not.

## Platform

This is a Rust workspace (`edition` 2024, `rust-version` 1.88). The toolchain is Windows-only: `rust-toolchain.toml` pins `1.98.1` and `x86_64-pc-windows-msvc`. `victory-gpu` and `victory-hle` are `#[cfg(windows)]`. A Linux `cargo check` does not link them and is not a green GPU run.

Do not write a frame time, an fps number, or a "passed" row into `logs/results.md` or `README.md` unless that frame was captured on the target GPU. This host's reference card is an **RX 6650 XT 8 GB** (Ryzen 5 8400F, 32 GB RAM, Windows 11 `10.0.26300`); older log rows may still mention RX 6600 XT / 16 GB from prior machines.

## What CI actually runs

`.github/workflows/ci.yml` is one job, `check`, on `windows-latest`:

```
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

The two `cargo test` lines in the README are a subset of this, not a substitute. This fork has not been running Actions; a green local claim needs the Windows commands above.

## Asset policy

`cargo xtask policy` is the check, and `repository_passes_policy` in `xtask` runs it on the whole tree. Install the hook with `cargo xtask install-hooks`.

A file passes only with an allowlisted extension (`rs`, `toml`, `md`, `mdc`, `txt`, `lock`, `yml`, `yaml`, `json`, `hlsl`, `hlsli`, `ps1`, `cmd`, `sh`, `svg`) or an allowlisted name, under 512 KiB, with no NUL bytes, no known binary signatures, and no hex/base64 run longer than 512 characters. The only exception is `crates/victory-formats/src/bcpack-tables.bin`. A new extension needs an entry in `xtask/src/policy.rs`.

Do not commit game files, dumps, shader dumps, screenshots, or `victory.toml`. Paths to those live outside the repository. `.victory-root` plus `victory_core::ensure_outside_repo` refuse derived data written inside the tree. A leak is removed with `git filter-repo`, not with a follow-up commit.

## Where code goes

`crates/victory-game` is the title: offsets, signatures, containers. Every other crate is the platform. Do not put title bytes into literals or fixtures. Tests that need real files read `VICTORY_GAME_DIR`, `VICTORY_CONSOLE_DXC`, `VICTORY_PC_DXC`, and `VICTORY_WARP`, and skip when those are unset.

## Style and build

`rustfmt.toml` sets `max_width` 100. CI denies clippy warnings. `Cargo.toml` already allows `type_complexity`, `needless_range_loop`, and `undocumented_unsafe_blocks` (plus a few more). A new warning still fails CI.

`.cargo/config.toml` passes `-C target-cpu=x86-64-v3` only for `x86_64-pc-windows-msvc`. Release uses `lto = "thin"` and `codegen-units = 1`, so a release build is slow. Profile `diag` is the diagnostics build. `.gitattributes` forces LF, except CRLF for `*.cmd` and `*.ps1`.

## Changing behavior

Leave the default path alone. A new speedup is a flag that defaults to the old path until there is a captured frame and a clean D3D12 Debug Layer run. Do not replace the incremental `heap_buffers` walk (`VICTORY_HEAP_RESCAN_FRAMES`, a rotating 1/N slice) with a full scan.

## Commits and branches

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>[optional scope][!]: <description>
```

`feat` and `fix` are the required types. Also use `perf`, `docs`, `refactor`, `test`, `build`, `ci`, and `chore`. The subject is lowercase and has no trailing period. A breaking change takes `!` after the scope, or a `BREAKING CHANGE:` footer. Say in the body when the change was not run on Windows.

Branch names follow [Conventional Branch](https://conventionalbranch.org/) 1.1.0: `<type>/<description>`, lowercase, hyphen-separated. No underscores, no uppercase, no doubled or leading or trailing hyphens.

Purpose types: `feature/` (or `feat/`), `bugfix/` (or `fix/`), `hotfix/`, `release/`, `chore/`. `main` takes no prefix. A docs or dependency change is `chore/`, not `docs/`. The branch states the purpose of the work. Individual commits on it still use their own Conventional Commits types, including `perf` and `docs`.

Examples: `feature/heap-skip-tabled`, `fix/header-bug`, `chore/agents-md`, `release/v1.2.0`.

An agent that opens the branch itself prefixes it with its own name: `cursor/`, `claude/`, `codex/`, `copilot/`, or the vendor-neutral `ai/`. The description after that prefix follows the same grammar.
