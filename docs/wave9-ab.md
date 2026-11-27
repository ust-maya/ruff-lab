# Wave 9.0 — independent A/B runs (RX 6650 XT)

Do **not** bundle these two experiments. Each run uses the README Series S preset +
`VICTORY_LOW_GRAPHICS=1` + `VICTORY_FFX_SIMPLE=1` + scripted `VICTORY_KEYS` past account select.
Baseline for comparison: [logs/results.md](../logs/results.md) row **#12** (heavy ≈ **8.1** fps,
`draws≥2500`, `ms≤2 s`, `n≥30`).

No quit-after-N-frames env: force-stop once you have ≥~30 heavy frame lines (~3–5 min in-world).
Do **not** write fps into `logs/results.md` / `README.md` until the run is on this RX 6650 XT.

Shell: PowerShell from `G:\GTA6\ruff-lab`. Redirect stderr for frame lines, e.g.
`Start-Process ... -RedirectStandardError G:\GTA6\victory-data\wave9-<tag>.log.err`.

## Shared preset (paste first)

```powershell
cd G:\GTA6\ruff-lab
$game = 'G:\GTA6\GTA6.Xbox.v1.0.2.1'
# VICTORY_KEYS=1 alone does NOT press Enter. Format is "<seconds>:<key>,..."
# (see gameinput.rs scripted()). Enter every 7s + Down/Enter every 35s for ~20 min:
$keyParts = @()
foreach ($t in 3..1200) {
  if ($t % 7 -eq 0) { $keyParts += "${t}:Enter" }
  if ($t % 35 -eq 0) { $keyParts += "${t}:Down"; $keyParts += ("{0}:Enter" -f ($t + 0.4)) }
}
$env:VICTORY_KEYS = ($keyParts -join ',')
$env:VICTORY_ONE_PAD = '1'
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
$env:VICTORY_LATE_WAIT = '3'
$env:VICTORY_RELAY_COPY_QUEUE = '0'
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_WIDE_MERGE = '1'
$env:VICTORY_WIDE_REFRESH = '1'
$env:VICTORY_WIDE_THREADS = '6'
$env:VICTORY_UNDER_WRITES_WORDS = '0'
$env:VICTORY_UNDER_WRITES_RUNS = '100000'
$env:VICTORY_UNDER_WRITES_MASKS = '1'
$env:VICTORY_UNDER_WRITES_MASKS_LEAST = '1'
$env:VICTORY_CARD_FETCH_INFLIGHT = '1'
$env:VICTORY_COPY_CHANGED = '1'
$env:VICTORY_RANGED = '1'
$env:VICTORY_HEAP_RESCAN_FRAMES = '64'
$env:VICTORY_CARD_BUFFERS_MB = '1024'
$env:VICTORY_CARD_REHASH_RUNS = '1'
$env:VICTORY_JOIN_COPIES = '1'
$env:VICTORY_DSTORAGE_WORKERS = '3'
$env:VICTORY_PIPELINE_WORKERS = '4'
$env:VICTORY_PIPELINE_WAIT_MS = '250'   # override only in experiment B
$env:VICTORY_COMMIT_RESERVE_MB = '2048'
$env:VICTORY_REMAP_LEAN = '1'
$env:VICTORY_TEXTURE_MAX_DIM = '1024'
$env:VICTORY_TEXTURE_LRU = '1'
$env:VICTORY_TEXTURE_ROOM_PERCENT = '92'
$env:VICTORY_TEXTURE_KEEP_PERCENT = '78'
$env:VICTORY_DYNAMIC_SLOTS = '524288'
$env:VICTORY_DISPLAY_MODE = '1280x720'
$env:VICTORY_AMBIENT_SHADE = '0'
$env:VICTORY_FFX_SIMPLE = '1'
$env:VICTORY_LOW_GRAPHICS = '1'
# leave IDLE_LISTS / LIST_CEILING unset on this card
```

## Experiment A — Wave 7 all-on vs #12

```powershell
# after shared preset:
$env:VICTORY_HEAP_SKIP_TABLED = '1'
$env:VICTORY_HEAP_SYNC_SKIP = '1'
$env:VICTORY_HEAP_SYNC_BATCH = '1'
$env:VICTORY_COPY_CHANGED_DIRECT = '1'
.\target\release\victory.exe run --console series-s "$game\game_scarlett_master.exe" --execute
```

Expect frame-line counters `tabled slots not re-mirrored` / `pages not read`. Compare heavy fps to #12.

## Experiment B — only `PIPELINE_WAIT_MS=0` vs default 250

```powershell
# after shared preset (Wave 7 flags UNset):
Remove-Item Env:VICTORY_HEAP_SKIP_TABLED,Env:VICTORY_HEAP_SYNC_SKIP,Env:VICTORY_HEAP_SYNC_BATCH,Env:VICTORY_COPY_CHANGED_DIRECT -ErrorAction SilentlyContinue
$env:VICTORY_PIPELINE_WAIT_MS = '0'
.\target\release\victory.exe run --console series-s "$game\game_scarlett_master.exe" --execute
```

## Wave 9.1 opt-ins (default OFF — separate later A/B)

| Flag | Default | Effect |
|---|---|---|
| `VICTORY_LIBRARY_STORE_LATER=1` | **broken / ignored** | Was: defer `StorePipeline` during save. **#19 device removed**; `stores_later()` always false + warn |
| `VICTORY_SELECTIVE_UAV_WRITEBACK=1` | off | Skip root-UAV writeback for slots classified read-only (#3548 half) |
| `VICTORY_BFI=1` | off | Lower `s_bfm_b32` / `v_bfm_b32` with DXIL `Bfi` (misses warm shader/pipeline cache) |

Do not enable these in the 9.0 Wave 7 / WAIT_MS runs above. Do not set `LIBRARY_STORE_LATER`.

## Experiment E — only `SELECTIVE_UAV_WRITEBACK=1` vs #12 / #20

```powershell
# after shared preset (Wave 7 / WAIT_MS override / LIB_STORE / BFI UNset):
Remove-Item Env:VICTORY_HEAP_SKIP_TABLED,Env:VICTORY_HEAP_SYNC_SKIP,Env:VICTORY_HEAP_SYNC_BATCH,Env:VICTORY_COPY_CHANGED_DIRECT,Env:VICTORY_LIBRARY_STORE_LATER,Env:VICTORY_BFI -ErrorAction SilentlyContinue
$env:VICTORY_PIPELINE_WAIT_MS = '250'
$env:VICTORY_SELECTIVE_UAV_WRITEBACK = '1'
.\target\release\victory.exe run --console series-s "$game\game_scarlett_master.exe" --execute
```

## Live results (RX 6650 XT, 2026-11-28)

| Run | Heavy avg/med/peak | Settled | Relay wait | Verdict |
|---|---|---|---|---|
| #12 baseline | 8.1 / 8.9 / 10.1 (`n=74`) | 8.7 fps / 116.5 ms | 34.9 ms | reference |
| **A** Wave 7 all-on (`#17`) | **7.5 / 7.7 / 9.2** (`n=51`) | 7.9 / 127.8 | ~52 ms | mild regression; **stop Wave 7** |
| **B** `WAIT_MS=0` (`#18`) | **8.3 / 8.7 / 9.4** (`n=60`) | 8.8 / 113.9 | **10.7 ms** | slight win / parity; pursue |
| **C** `LIBRARY_STORE_LATER=1` (`#19`) | n/a | — | — | **device removed ~2.5 min**; path **disabled in code** |
| **D** new-bin control, flags off (`#20`) | **9.2 / 9.6 / 10.2** (`n=51`) | 9.7 / 103.6 | ~9 ms | new exe OK; isolates C |
| **E** `SELECTIVE_UAV_WRITEBACK=1` (`#21`) | **9.5 / 10.2 / 10.9** (`n=75`) | 10.2 / 98.1 | ~38 ms | mild win vs #12; parity/#20; **keep opt-in** |

A/B used pre-9.1 exe. C/D/E used rebuilt exe with 9.1 code (E alone). `LIBRARY_STORE_LATER` env ignored (`stores_later` always false). Next: pursue `WAIT_MS=0`; keep `SELECTIVE_UAV` opt-in; BFI still deferred (no in-world yet; cache miss); no Wave 7.
