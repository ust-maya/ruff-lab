<h1 align="center">ruff-lab</h1>

<p align="center">
  <b>An open-source Xbox Series X|S (Scarlett) compatibility runtime fork tuned for mid-range PCs: 8 GB VRAM, 32 GB RAM, and mixed HDD/NVMe storage.</b>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/base-KLBF%20Lab%2039-1f46c8" alt="Base: KLBF Lab 39">
  <img src="https://img.shields.io/badge/GPU-AMD%20RX%206650%20XT%208GB-ed1c24" alt="GPU: AMD RX 6650 XT 8GB">
  <img src="https://img.shields.io/badge/RAM-32%20GB-1f46c8" alt="RAM: 32 GB">
  <img src="https://img.shields.io/badge/license-GPL--3.0-555" alt="GPL-3.0">
</p>

> [!NOTE]
> **No game files, dumps, or pre-built binaries are included in this repository.** You must supply your own legally obtained files and build from source.

---

## Status (2026-11-27)

**Stage 6 — In-game verified on this RX 6650 XT** with Wave 8 ports (`#3177`/`#2706`/`#3292`): prison cell + yard live (~11.5 min).

Tested on this PC: **Windows 11 Pro (`10.0.26300`)** · **AMD Radeon RX 6650 XT 8 GB** (RDNA2, driver `32.0.21045.29003`) · **AMD Ryzen 5 8400F** (6C/12T) · **32 GB RAM** · pagefile on **C:** (**ADATA LEGEND 900** NVMe) · game/build on **G:** (Storage Spaces over **R5MP480G8** NVMe, ~447 GB) · also **ST1000DL002** 1 TB SATA HDD · PowerShell.

**Paths on this PC**

| Role | Path |
|---|---|
| Git repo / build | `G:\GTA6\ruff-lab` |
| Game dump (`victory.toml` → `game_dir`) | `G:\GTA6\GTA6.Xbox.v1.0.2.1` |
| Standalone exe copy (no git) | `G:\GTA6\MAIN EXECUTABLE\game_scarlett_master.exe` |

| Lane / Stage | Status | Details |
|---|---|---|
| **Boot & D3D12 init** | ✅ Works | Native Wave64 (`min=32 max=64 compute=64`); live 2026-11-27 on RX 6650 XT: D3D12 device ready, title window up; unit Debug Layer coverage historically green (see prior rows) |
| **Setup, menus & pause UI (`series-s` + FFX simple)** | ✅ Works | Live 2026-11-27: account-select / load UI at **30.0 fps** (`33.3 ms/frame`, `14` draws) with `VICTORY_LOW_GRAPHICS=1`; earlier runs also had **22 fps** pause menu |
| **Offline pipeline pre-build** | ✅ Works | `27,144 / 27,144` pipelines built in `1,029 s` with **0 `amdxc64.dll` crashes** |
| **Player control (Stage 6)** | ✅ Verified | Earlier: **native DualShock 4 (`0x09CC`)** + `VICTORY_ONE_PAD=1`. Live Wave 8 2026-11-27: no physical pad; **`VICTORY_KEYS` Enter** + keyboard stand-in cleared account select and reached prison cell (`L`/`R` move/look prompts on screen) |
| **Picture (in-world)** | 🟢 Natural lighting & textures | Wave 6 `ambient.rs` enables balanced sky-dome-following ambient by default (`gain=0.44, least=0.075, power=0.36`, unit-luma night tint, capped at `0.6`, `0.28` pre-sky fallback, `reflections held=0.5`, `VICTORY_AMBIENT_SKY=off` reverts to constant `0.6`), eliminating both night auto-exposure white blowout and indoor dawn silhouettes; 79 card-resident buffers (`657 MB`, `0.0 MB` CPU spill); Wave 4 fixed all `226 -> 0` refused raw buffer views (`own_bytes`) and 9-bit `exp prim`; Wave 5 adds 16-bit `half_views` (NPC facial animation network + `ped_standard_head_cs`), BC7 mode 4/5 rotation fix, RDNA 2 `v_fmac_f32` accumulator `abs`/`neg`, `ForceEarlyDepthStencil`, `VICTORY_D16_BITS`, `VICTORY_SPILL_STRICT`, and aliased texture re-copy |
| **Speed (in-world)** | 🟢 **Wave 8 + phase-2 baseline on this RX 6650 XT** | Live 2026-11-27 `b1d91be` baseline (new opt-ins **unset**) + `LOW_GRAPHICS=1` + `FFX_SIMPLE`: heavy **`8.1 / 8.9 / 10.1` fps** (`n=74`, draws≥2500, `ms≤2 s`); settled-ish **`8.7 fps / 116.5 ms`**. Earlier Wave 8 (`bba4f3d`): heavy **`5.2 / 4.3 / 11.6`**. **Do not enable** `VICTORY_IDLE_LISTS` / `VICTORY_LIST_CEILING` on this card — alone they drop heavy to **`3.7 / 4.1 / 4.4`** (abbr. run); full new-flag bundle worse still (**`2.2 / 2.2 / 3.0`**). Menu 30 fps is **not** in-world. |
| **Stability** | ✅ **30–39 min** runs without a crash | Clean exits, `0 ERROR`, `0` unhandled exceptions, `0` device removals, `0` `amdxc64.dll` crashes; walking `E_INVALIDARG` crash fixed (`cf489e1`); pagefile moved to **M.2 NVMe SSD** |

Full session-by-session logs: [logs/results.md](logs/results.md).

---

## What `ruff-lab` adds on top of KLBF Lab 39

Most compatibility forks are tested on 32 GB RAM and 12–16 GB VRAM cards. `ruff-lab` focuses on **8 GB GPUs** (this host: RX 6650 XT + 32 GB RAM) and still keeps the older **16 GB RAM / HDD** mitigations:

| Area | Problem on 8 GB / 16 GB / HDD | Fix in `ruff-lab` |
|---|---|---|
| **8 GB VRAM texture starvation** | `targets_room_kept()` hardcoded a `3,072 MiB` reserve (measured on 16 GB Series X runs), leaving only ~2.5 GB for textures on 8 GB cards (`no room for new ones so_far=25500`), and refusing textures at 90% VRAM left ground, hands, and character skin pitch black | Adaptive `default_targets_room_mb()` (14% / max `1,024 MiB` in Series S 720p), mip-dropped short-room fallback (`VICTORY_TEXTURE_SHORT=256` up to 97% VRAM, `ka1qux`), world-texture max-dimension cap with UI protection (`VICTORY_TEXTURE_MAX_DIM=1024` + `KEEP_UI`, `klbf-lab` Lab 62), LRU eviction (`VICTORY_TEXTURE_LRU=1`), and buffer-pass heap-drop cleanup (`VIniVIdiVIci`) |
| **Sky-dome AMD lighting & RT pass removal** | Without hardware RT, `CS_ScreenProbes_TemporalResolve` left bright magenta and pitch-black shadows; unused RT passes and ray-request feeders wasted GPU time and allocated RT-only textures | Upgraded `ambient.rs` from [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) (`4f62c2a`) so ambient light follows the sky dome dynamically (day/night, `VICTORY_AMBIENT_SKY=off` to keep constant) + clamps unoccluded reflections (`0.5`), and added `VICTORY_SKIP_RT=1` (default ON) skipping unused RT & ray-feeder compute passes (`Belobug` `6986a57` + `vilocity` `2681546`) so they cost 0 ms and allocate 0 RT textures |
| **16 GB RAM staging bloat** | Every sampled texture kept a full-size staging copy in system RAM, pushing commit to 12–20 GB and thrashing pagefiles | Integrated [PastiiJay/klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23) (by `spy0eye-beep`): releases staging buffers of unchanged textures after 2 s / 30 frames while keeping metadata |
| **6-core CPU descriptor lock contention & heap limits** | Global mutex on `HostHeap::mirror` stalled multi-threaded descriptor updates, descriptor tables were re-scanned every draw, and fixed 256K dynamic slots dropped draws on AMD's 2M view heap | Integrated [PastiiJay/klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16) (256 cache-aligned stripes + lock-free fast path), `VICTORY_DYNAMIC_SLOTS` (`klbf-lab` `4e95986`), pre-grouped `TablePlan` once per pipeline (`coolfox`/`VIctory-7900XTX` `46e3297`), one-time heap-cap warning ([VIctory#2634](https://github.com/Marvisak/VIctory/pull/2634)), and 64 KiB CBV cap with 0-byte CBV preservation |
| **Rotational HDD seek thrashing** | DirectStorage spawned `available_parallelism()` (12) concurrent file reader threads | Added `VICTORY_DSTORAGE_WORKERS` (`1..=64`, default = CPU count) in `crates/victory-hle/src/modules/dstorage.rs` so HDD users can set `2`–`4` workers |
| **Native Sony DS4 / DualSense input** | DualShock 4 & DualSense controllers required `x360ce` / `ViGEmBus` / `DS4Windows`, which often broke or failed to bind to `GameInput` | Native Windows HID reader (`crates/victory-hle/src/modules/gameinput/sony.rs`, adapted from [realblastex/flamingo](https://github.com/realblastex/flamingo)) supporting **DualShock 4** (v1/v2/dongle) & **DualSense / Edge** over USB and Bluetooth as slot 0 fallback (`VICTORY_SONY_PAD=0` to disable) |
| **BCPack fragment edge cases** | Corner-case single-fragment / zero-count BCPack textures could fail decompression | Backported [Marvisak/VIctory#2648](https://github.com/Marvisak/VIctory/pull/2648) & [#2649](https://github.com/Marvisak/VIctory/pull/2649) in `crates/victory-formats/src/bcpack.rs` |
| **Release binary CPU throughput** | Default release profile compiled for baseline SSE2 without LTO across crates | Enabled `lto = "thin"`, `codegen-units = 1` ([Belobug#96](https://github.com/Belobug/VIctory-Belobug-AMD/pull/96)), and `target-cpu=x86-64-v3` (AVX2/BMI2/FMA) on Windows ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)) |
| **Refused AMD shaders & RDNA 2 correctness** | 40+ shaders failed translation on uniform static-sampler branches or split wave64 lane masks; RDNA 2 `v_mad_legacy_f32` and `v_cmpx_*` VOP3/SDWA needed exact GFX10+ semantics | Ported static-sampler branch selection, integer-target blend bypass (`ddb73326b0`), split wave64 lane-mask recovery (`76749ce58d`) from [DexNom/VIctory](https://github.com/DexNom/VIctory), RDNA 2 fused `v_mad_legacy_f32` + FTZ (`victory-fork` `f6a09d3`), GFX10 `v_cmpx_*` EXEC decoding ([VIctory#3083](https://github.com/Marvisak/VIctory/pull/3083)), and 3D `DispatchMesh` (`VIctory#2624`) |
| **16 GB RAM commit pressure & pipeline workers** | Background pipeline workers spawned 6+ threads (~3–5 GB extra RAM), slow compute PSOs starved graphics draws, and normal exits could lose recent PSO cache writes | Added `VICTORY_PIPELINE_WORKERS` ([VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)), graphics-first worker priority reserving a worker from compute ([VIctory#3025](https://github.com/Marvisak/VIctory/pull/3025)), pipeline cache flush on normal exit ([VIctory#3046](https://github.com/Marvisak/VIctory/pull/3046)), and `VICTORY_COMMIT_RESERVE_MB` (`ka1qux`) |
| **CPU frame time: copies, hashing, binds & barriers** | `ReadProcessMemory` kernel crossings (`37–52 ms/frame`), `blake3` per-refresh texture hashing, repeated pipeline/RT/CBV/GPU-write declarations per list, per-buffer barriers, view-guard scans, and page remaps | Ported `plaincopy.rs` (`kelvaroq` `0044`), `xxh3-128` texture hashing ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)), `bound.rs` ([VIctory#3167](https://github.com/Marvisak/VIctory/pull/3167)), render-target bind cache (`VICTORY_BIND_CACHE`), root CBV scratch cache, `VICTORY_DECLARE_ONCE` (`cgbj`/`Belobug`/`ZJOU`), 8-way card compare (`9b90ad1`), `WaitOnAddress` fence wakes, XMA audio refill fix (`cgbj`), batched card-copy barriers (`773d902`), interval `CleanBlocks` + `Uncommitted` memo (`edsuuu`), `VICTORY_REMAP_LEAN=1` (`ka1qux`), and +27% BC7 BCPack lookup tables ([VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735)) |
| **Waves 1–2: GPU settles, copy-back & masked uploads (`3f43861`, `a4fb0c7`, `bcca048`)** | `waiting for the card` (`126.6 ms/frame`), `copying back` (`132 ms`), relay fetch waits (`49.5 ms`), 351k tiny `CopyBufferRegion` calls/frame, 16-frame full 75k-slot `heap_buffers` rescan spikes, and `scene_burst` spam (`831/run`) | Added `CARD_PATCH` HLSL compute shader (`VICTORY_UNDER_WRITES_MASKS=1`, collapsing 351k tiny copies into 1–2 masked 4 KiB dispatches, D3D12 Debug Layer verified byte-exact on RX 6600 XT), `VICTORY_RANGED`, `VICTORY_COPY_CHANGED`, `VICTORY_CARD_FETCH_INFLIGHT=1` (`0.0 ms` fetch waits), `VICTORY_WIDE_THREADS`, rotating `1/64` incremental `heap_buffers` rescan (`VICTORY_HEAP_RESCAN_FRAMES=64`), shared stencil depth-twin scratch pool ([VIctory#2162](https://github.com/Marvisak/VIctory/pull/2162)), 500 ms `E_OUTOFMEMORY` retry backoff ([VIctory#3276](https://github.com/Marvisak/VIctory/pull/3276)), `SHADER_HEAPS` `RwLock` ([VIctory#2821](https://github.com/Marvisak/VIctory/pull/2821)), and parallel `sync_heaps()` |
| **Wave 3: Multi-threaded descriptor/heap CPU & HDD streaming (`c0a3a1f`)** | Single-threaded 35k-slot `heap_buffers` (`103–129 ms/frame`), double-enqueue of `self.changed` on `seen=true`, mutex contention on `placed`/`textures` in `sync_heaps()`, per-block `VirtualProtect`, per-draw `user_data` heap allocs, and serialized HDD reads (`FILE_FLAG_NO_BUFFERING` + sync DirectStorage handle) | Fixed `self.changed` re-enqueue on `seen=true`, added lock-free per-slot `slot_flags` (`SLOT_PLACED`/`SLOT_TEX`), parallelized `heap_buffers()` slot mirroring via `std::thread::scope`, added stack `[0u32; 64]` `user_data`, `HostList::states_generation` memoization ([VIctory#3249](https://github.com/Marvisak/VIctory/pull/3249) + `d4903fa`), lock-free `Shadow::pending_len`, pooled copy-back scratch & non-allocating `Shadow::writing` ([VIctory#2848](https://github.com/Marvisak/VIctory/pull/2848)/[#3318](https://github.com/Marvisak/VIctory/pull/3318)), coalesced `VirtualProtect` with 487 fallback, OS file cache for sync read-only packs (`VICTORY_PACK_CACHE`), and `FILE_FLAG_OVERLAPPED` concurrent DirectStorage reads ([VIctory#2427](https://github.com/Marvisak/VIctory/pull/2427)) |
| **Wave 4: Visual, shader & stability bugfixes (`cf489e1`, `b3073ea`, `3ea0d32`)** | `E_INVALIDARG` walking crash on mip-dropped textures, DS4 reconnect pause, 226 unaligned raw buffer views (`format=39 first=2 / 1615`) left `null`, GFX10 `exp prim` 10-bit mask culling valid mesh triangles, waterfall `s_andn2_wrexec` dropping 2 compute shaders, invalid IB leaks, worker panics, DSP race, and stale cache `.tmp` after kill | Fixed mip-dropped copy bounds (`cf489e1`), `VICTORY_ONE_PAD=1`, scene preload gate + monotonic clock freeze (`b3073ea`), `own_bytes()` 16-byte alignment copy for unaligned raw SRV/UAV (`226 -> 0` null views), 9-bit `exp prim` mask `0x1FF` (`c0228d9`), waterfall `s_andn2_wrexec` `SCC=false` ([VIctory#3016](https://github.com/Marvisak/VIctory/pull/3016)), IB validation ([VIctory#3183](https://github.com/Marvisak/VIctory/pull/3183)), pipeline worker `catch_unwind` guard (`#1353`), atomic `Status` + `Mutex` in `dsp.rs` ([VIctory#3319](https://github.com/Marvisak/VIctory/pull/3319)), and `.create(true).truncate(true)` cache staging (`#1321`) |
| **Wave 5: Kitty6 ports — lock-free lookups, memory pools, RDNA 2 & NPC facial animation** | `shadow::exact` global lock stalling recording threads, `LOOSE` global `Mutex`, unsized root views faulting past 4 MiB on AMD GPUs, snapshot `POOL` holding up to 512 MiB idle, per-list read-back buffer churn & streaming lock stalls in `texback::land`, `Vec<bool>` dynamic slot scans, BC7 non-mode-4/5 channel swap, missing `v_fmac_f32` accumulator modifiers on RDNA 2, glass prepass overwriting characters, and frozen/garbled NPC faces from float-typed `d16` / `R16_UINT` buffer loads | Ported AMD-safe & 16 GB RAM-tuned improvements from [kitty-core/Kitty6](https://github.com/kitty-core/Kitty6) and upstream: `shadow::exact` allocation outside lock + 64 MiB reuse pool (`VICTORY_EXACT_REUSE`, `9eb362c`), `LOOSE` `RwLock` (`42046f5`), `root_window` committed reach up to `32 MiB` (`VICTORY_ROOT_WINDOW_MB`, `5d622a1`) + 1 s `VirtualQuery` cache (`VICTORY_ROOT_REACH_MS`, `33c5888`), snapshot `POOL` idle sweep & 32 MiB RAM-pressure shrink (`VICTORY_SNAPSHOT_POOL_IDLE`, `9ea429c`), RAM-pressured 5 s uncapped texture eviction (`VICTORY_TEXTURE_PRESSURE_IDLE`, `e29ee1a`), power-of-two `texback` read-back pool (`VICTORY_READBACK_POOL`, 128 MiB cap, `43c2f3f`) + async coalescing writer thread (`VICTORY_TEXBACK_ASYNC`, `eeb6e35`), 64-bit word `Taken` dynamic slot bitmap (`d760ccb`) + early table release (`VICTORY_EARLY_TABLES`, `9cc794d`), `texture::alias` source-change tracking (`cd537b7`), BC7 rotation restricted to modes 4–5 (`e1fd1f9`), RDNA 2 `v_fmac_f32` VOP3 bit-2 `abs`/`neg` (`fdca008`), wave section scan past unmatched `saveexec` (`d81f488`), `VICTORY_EARLY_DEPTH` / `VICTORY_D16_BITS` / `VICTORY_SPILL_STRICT` + `s_bfm_b32` (`c0f8b6d`), and full 16-bit `half_views` support (`emit/halves.rs`, `68cef8b` + `ee7694e` + `6779c11` + `a20904c`) restoring NPC facial blendshapes |
| **Wave 6: CPU bottleneck elimination (`heap_buffers` / `sync_heaps` / `settles`) & night overexposure fix** | Per-draw buffer/CBV table slots weren't marked `tabled`, causing `sync_heaps` (`26–79 ms`) to re-mirror 20k–56k slots/frame and flood `heap_buffers` (`110–175 ms`); global `Mutex` locks on `buffers`/`changed`/`textures`/`RETIRED`; `heap_textures` ran with a `1000 ms` budget during live gameplay (`14–42 ms/frame`); coarse `written` ranges caused `Step::Fill` CBV uploads to stall on GPU settles (`24–58 ms`); `copy_changed` skipped >16 MiB unwatched ping-pong buffers; constant `0.6` ambient blew out night/car scenes under auto-exposure | Marked per-draw table buffer & CBV slots tabled (`VICTORY_TABLED_BUFFERS`, default ON), replaced `buffers`/`changed` with lock-free atomic `Slots` bitsets & masked round-robin `part_of` (`Kitty6` `ab9f1b7`), added lock-free `Queued` texture bitset + out-of-lock `TextureQueue::in_order` + 4096 slot-lock stripes + 16-sharded `RETIRED` (`Kitty6` `11511b8`), skipped failed `GetWriteWatch` on known views (`Kitty6` `65f9707`), capped live `heap_textures` at `10 ms` / `256 MB` (keeping full preload gates), added `watch::note_landed`/`landed_lately` + `VICTORY_FILL_TITLE=2` (`Kitty6` `11511b8`/`b6be7e3`) + opt-in `VICTORY_FILL_STALE_BYTES`, chunked `Shadow::copy` diff uploads in `2 MiB` slices for >16 MiB unwatched buffers (`Kitty6` `60532f0`), enabled `VICTORY_WIDE_REFRESH` / `VICTORY_WIDE_MERGE` / `VICTORY_CARD_REHASH_RUNS` / `VICTORY_INDIRECT_WINDOW` by default, and calibrated default sky-following ambient (`0.44,0.075,0.36`, unit-luma night tint, `0.6` cap, `0.28` pre-sky fallback) in `ambient.rs` (zero shader-cache invalidation) |
| **Wave 6.5: GPU texture detiling, black/blocky face fixes & slow-compute thinning** | CPU texture detiling ate Ryzen 5 3600 cycles during streaming; placed textures uploaded mid-read or painted through alias views (NPC faces/hair) stayed black or blocky; dynamic slots could release into a reused heap address; single-group compute shader `02384a60fda6f2d0` stalled the GPU up to `~10.9 ms/frame` | Ported GPU compute detiling (`gpudetile.rs`, `VICTORY_GPU_DETILE=1` default ON, adapted from `fxceless/victory-fork` `3461f52`) with mip-drop (`dropped > 0`) support and a `64 MiB` VRAM pool integrated into `klbf-lab#23` idle staging eviction; ported `VICTORY_PAGE_WRITES` default `1\|2\|8`, `HostHeap::id` dynamic-slot ownership guard, full `xxh3-128` fingerprinting & periodic recheck for aliased textures (`VICTORY_FULL_PRINT_KB`/`VICTORY_ALIASED_RECHECK_EVERY`, `Kitty6` `aca0877` + `ae3cce8`); thinned `02384a60fda6f2d0` to every 4th frame (`VICTORY_THIN_SLOW_COMPUTE=4`, `Kitty6` `7124a8f`); and added opt-in `VICTORY_INPLACE_CONSTANTS` (`fxceless` `bcc5421`, default OFF for PCIe x8 GPUs) — zero shader-cache invalidation |
| **Wave 6.6: WDDM L0 shared-memory OOM fix, root-window deduplication & texture-pass hotfix** | On 16 GB RAM PCs (WDDM L0 shared GPU memory cap `~7.5 GiB`), `root_window` created overlapping 32 MiB windows every 4 MiB cell (`shared` grew `994 MB -> 6,561 MB+` -> `E_OUTOFMEMORY 0x8007000E` after ~23 min and duplicated `copies` up to `185.9 MB/frame`); `fingerprint_texture` switched between `blake3` and `xxh3-128` whenever alias state toggled (`38,001` false re-uploads / `23.3 GB` churn); `heap_textures` called `resource::let_go_idle()` every frame when VRAM was full (`22–28.9 ms/frame`); and `E_OUTOFMEMORY` aborted without draining idle L0 pools | Deduplicated `LOOSE` root windows via `reuse_root()` + `find_any` reuse + bounded `find_cbv()` for 64 KiB root CBVs and scaled default `VICTORY_ROOT_WINDOW_MB` to `16 MiB` on `<= 20 GiB` RAM PCs; tagged fingerprint algorithm in bit 1 (`recheck_fingerprint`, zero false re-uploads) and bounded aliased recheck (`1024 KiB` / `120` frames); rate-limited emergency `resource::let_go_idle()` in `heap_textures` to 1 s and timed the full pass against `VICTORY_TEXTURE_STAGE_MS`; pooled GPU-detile upload buffers (`SPARE_UPLOAD`, 32 MiB cap); added WDDM `NON_LOCAL` shared-memory pressure tracking (`shared_memory_pressure`) driving `ram_idle_ms`, `sweep()`, and pool trims (`SPARE`, `SPARE_UPLOAD`, `SPARE_LINEAR`, `texback::SPARE`, `shadow` pools, `list::IDLE`); and added serialized `reclaim_for_oom()` + 1-retry on `E_OUTOFMEMORY` across all D3D12 buffer/list/prelude allocators (zero shader-cache invalidation) |
| **Low-graphics / potato mode (`VICTORY_LOW_GRAPHICS=1` / `2`)** | Full shadow cascades, mirror/planar/water/SSR reflections, world voxelization, volumetric fog/clouds, 1024 px textures and 16x anisotropy multiply draw count, VRAM usage, and CPU bind overhead on mid-range PCs | Added `VICTORY_LOW_GRAPHICS=1` (low) and `=2` (ultra-low/potato) skipping shadow-map rasterization (`CascadeShadows`, `PS_shadow`, `VS_shadow`, `ParaboloidShadows`), mirror/water/GBuffer/SSR reflections, voxel GI, puddle/glare/heat passes (and volumetric fog/clouds/blur in `=2`) while keeping main GBuffer, `ps_sky_paraboloid_reflection`, `PS_ShadowRender`, lighting, and UI intact; clamps `ambient.rs` reflections to `0.02` & disables depth occlusion; drops textures to `128 px` / `64 px` (`+1.5` / `+3.0` sampler LOD bias, 1x anisotropy, UI protected); and defaults display mode to `960x540` / `640x360` (zero shader-cache invalidation) |
| **Wave 7: opt-in CPU cuts (`VICTORY_HEAP_SKIP_TABLED`, `VICTORY_HEAP_SYNC_SKIP`, `VICTORY_HEAP_SYNC_BATCH`, `VICTORY_COPY_CHANGED_DIRECT`)** | After Wave 6 the draw calls `HostHeap::mirror` before the `tabled` stamp, so `changed` still feeds `heap_buffers`. `sync_heaps` still reads every dirty 64 KiB watch page. Card-resident `copy_changed` copied the CPU side into a scratch buffer that `copy_mapped` then overwrote | Four flags, each **default off**. Live 2026-11-28 Stage 6 on this RX 6650 XT (all four `=1`, `LOW_GRAPHICS=1`): heavy **7.5 / 7.7 / 9.2** vs #12 **8.1 / 8.9 / 10.1** (`0 ERROR`); counters live (`tabled slots not re-mirrored`). **Mild regression — leave defaults off; do not push Wave 7 further.** Separate: `PIPELINE_WAIT_MS=0` alone → heavy **8.3 / 8.7 / 9.4**, relay **10.7 ms** (vs 34.9) |
| **Wave 8: physical 2 MiB sections, keyed pipeline library, amdxc crash guard** | One pagefile section for the title's 6.1 GiB pool made Windows' large-page rebuild walk hundreds of thousands of views per trade (map/unmap stalls). Translator edits wiped a warm `host-pipelines.bin`. A worker stuck inside `amdxc64` AV killed the process | Ported [VIctory#3177](https://github.com/Marvisak/VIctory/pull/3177) (`VICTORY_SECTION_MB`, default **2** on Windows / `0` under Wine; DirectStorage fills across sections in place), [VIctory#2706](https://github.com/Marvisak/VIctory/pull/2706) (pipeline cache under `pipelines/<SOURCE_HASH>/`, keep 2 newest), [VIctory#3292](https://github.com/Marvisak/VIctory/pull/3292) (`VICTORY_COMPILER_GUARD` default ON + `warm-bad.txt`; `=0` disables). Skipped [#3035](https://github.com/Marvisak/VIctory/pull/3035) (this tree's `watch.rs` has no armed saved-byte path). Live 2026-11-27 RX 6650 XT **Stage 6**: `VICTORY_KEYS` past account select → Lucia cutscene → **prison cell/yard** (~11.5 min, `0 ERROR`); heavy in-world **`5.2 / 4.3 / 11.6` fps**, settled prison **`~8.8–9.2 fps` (`109–114 ms`)**; menu-only row 10 kept for boot evidence only |
| **Wave 9.1 opt-ins (`SELECTIVE_UAV_WRITEBACK`, `BFI`; `LIBRARY_STORE_LATER` disabled)** | Root-UAV writeback for read-only slots; `s_bfm`/`v_bfm` → DXIL `Bfi`; deferred `StorePipeline` during library save | All **default off**. Live 2026-11-28 RX 6650 XT: `SELECTIVE_UAV_WRITEBACK=1` alone (#21) heavy **9.5 / 10.2** — safe soft plus, keep opt-in. `LIBRARY_STORE_LATER` **device removed** (#19) → path forced off in code (`=1` warns). `VICTORY_BFI=1` opt-in only; **not** in-world yet; misses warm shader/pipeline cache. A/B recipes: [docs/wave9-ab.md](docs/wave9-ab.md); rows #17–#21 in [logs/results.md](logs/results.md) |
| **Phase-2 opt-ins (affinity, hoist, WC, lists, port constants) — keep default OFF** | Enabling the whole new bundle on RX 6650 XT tanked Stage 6 (~**8.1 → 2.2** heavy fps; relay wait **35 → 163 ms**) | Live A/B on `b1d91be` (`LOW_GRAPHICS=1`): **`IDLE_LISTS=64` + `LIST_CEILING=512` alone → ~3.7 heavy fps** (abbr.); `WC_NO_BACK` alone and `HOIST_CLAMPS=7` alone did **not** reproduce the cliff. Leave list caps unset; other new flags stay opt-in pending further short A/B. No built-in quit-after-N-frames — short A/B uses wall-clock kill once ≥~30 heavy samples |

---

## Building & Running

Requires Windows 10/11 x64, AVX2 CPU, D3D12 SM 6.6 + Mesh Shaders GPU, Rust (`1.98.1`, pinned in `rust-toolchain.toml`), MSVC C++ Build Tools, and [AMD FidelityFX SDK v2.4.0](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/releases) for Series S + FSR — or `$env:VICTORY_FFX_SIMPLE = '1'` for bilinear upscale without the SDK. Shell below is **PowerShell** from `G:\GTA6\ruff-lab`.

```powershell
cd G:\GTA6\ruff-lab
copy victory.example.toml victory.toml   # then set game_dir = 'G:\GTA6\GTA6.Xbox.v1.0.2.1'
cargo test -p victory-gpu --lib -- --test-threads=1
cargo test -p victory-hle --lib dstorage
cargo build --release -p victory
```

Pre-build pipelines once (recommended on AMD):

```powershell
$env:VICTORY_SHADER_OPT = '0'
$env:VICTORY_SKIP_PIPELINES = (Get-Content docs\klbf\skip-amd.txt -Raw).Trim()
.\target\release\victory.exe translate --cache --pipelines --jobs 8 'G:\GTA6\GTA6.Xbox.v1.0.2.1\shaders.rpf'
```

Run (`--console series-s`, tuned for **RX 6650 XT 8 GB / 32 GB RAM / Ryzen 5 8400F**; add `$env:VICTORY_FFX_DIR` for real FSR, or `$env:VICTORY_FFX_SIMPLE = '1'` without the SDK; optional `$env:VICTORY_LOW_GRAPHICS = '1'` or `'2'`; Wave 7 flags stay off unless set):

```powershell
$game = 'G:\GTA6\GTA6.Xbox.v1.0.2.1'
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
$env:VICTORY_PIPELINE_WAIT_MS = '250'
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
# optional: $env:VICTORY_LOW_GRAPHICS = '1'
# optional Wave 7 (default off): HEAP_SKIP_TABLED / HEAP_SYNC_SKIP / HEAP_SYNC_BATCH / COPY_CHANGED_DIRECT = '1'
# Wave 8 defaults: VICTORY_SECTION_MB=2 (Windows), COMPILER_GUARD on; set SECTION_MB=0 / COMPILER_GUARD=0 to revert
# Wave 9.1 opt-ins (default off): SELECTIVE_UAV_WRITEBACK / BFI = '1' (BFI misses warm shader cache)
# LIBRARY_STORE_LATER is broken (device removed #19) — env ignored in code; do not set
# Phase-2 opt-ins (default off). On this RX 6650 XT do NOT set IDLE_LISTS / LIST_CEILING — live regression (see logs/results.md #13/#16).
# No quit-after-N-frames env; for short A/B force-stop after ~30 heavy (draws≥2500) fps lines (~3–5 min once in-world).
# Wave 9.0 A/B recipes (Wave 7 bundle vs #12; WAIT_MS=0 alone): docs/wave9-ab.md
.\target\release\victory.exe run --console series-s "$game\game_scarlett_master.exe" --execute
```

---

## Bench

`victory bench` is a thin wrapper over `run --execute --console series-s` for
reproducible A/B measurements. It applies the Wave 9 shared preset (only for
variables still unset), generates scripted `VICTORY_KEYS` input, checks that
the pipeline cache is warm, and stops after N qualifying heavy samples
(`draws>=2500 && ms<=2000`). The GPU side writes a `victory-bench/v1` JSON
report outside the repository and exits the process; no manual kill is needed.

Prerequisite: warm the pipeline cache first (`translate --cache --pipelines`,
see above) — otherwise the bench run fails with
`run translate --cache --pipelines first`.

```powershell
cargo build --release -p victory
.\target\release\victory.exe bench 'G:\GTA6\GTA6.Xbox.v1.0.2.1\game_scarlett_master.exe' --frames 30
.\target\release\victory.exe bench 'G:\GTA6\GTA6.Xbox.v1.0.2.1\game_scarlett_master.exe' --frames 30 --out 'C:\bench\result.json' --set VICTORY_LOW_GRAPHICS=2
```

- `--frames 30` (default) counts qualifying heavy samples, not total frames.
- `--out <path>` writes the JSON there (must be outside the repository);
  without it the report lands in `<cache_dir>/bench/bench-<timestamp>.json`.
- `--set KEY=VALUE` overrides preset defaults (repeatable); variables already
  set in the environment win over the preset but lose to `--set`.
- No fps/frame-time numbers are recorded here: a number belongs in
  `logs/results.md` only when captured on the target GPU (see AGENTS.md).

Subsystem microbenchmarks (ignored tests, release build):

```powershell
cargo test --release -p victory-gpu --lib -- --ignored bench_mirror --nocapture
cargo test --release -p victory-shader bitcode -- --ignored --nocapture
cargo test --release -p victory-formats -- --ignored --nocapture
```

---

## Credits & License

- [Marvisak/VIctory](https://github.com/Marvisak/VIctory) — original Xbox Series X|S compatibility runtime for Windows.
- [fxceless/victory-fork](https://github.com/fxceless/victory-fork) — renderer and D3D12 work.
- [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab) / [klbf-lab-beta](https://github.com/PastiiJay/klbf-lab-beta) — Lab 39 AMD fixes, Series S HUD, water physics, and FSR frame generation.
- [realblastex/flamingo](https://github.com/realblastex/flamingo) — native DualSense HID reader design adapted for DS4 + DualSense in `gameinput/sony.rs`.
- [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) — sky-dome-following ambient & reflection clamp pass (`ambient.rs`, `4f62c2a`), RT screen-probe activation skip (`6986a57`), and release LTO optimization (`#96`).
- [DexNom/VIctory](https://github.com/DexNom/VIctory) — uniform static-sampler branch lowering, integer-target blend bypass (`ddb73326b0`), and split wave64 lane-mask recovery (`76749ce58d`).
- [ka1qux/BRVIctory-ka1qux](https://github.com/ka1qux/BRVIctory-ka1qux) — short-room mip-dropped textures (`VICTORY_TEXTURE_SHORT` / `VICTORY_TEXTURE_DROP_MIPS`), Windows commit reserve (`VICTORY_COMMIT_RESERVE_MB`), lean page remaps (`VICTORY_REMAP_LEAN=1`, after `victory-fork#1069`), and 3D `DispatchMesh` forwarding (`VIctory#2624`).
- [fablinoo/kelvaroq](https://github.com/fablinoo/kelvaroq) — in-process `rep movsb` + VEH fault-safe host copies (`plaincopy.rs`, `0044-copy-plain.patch`).
- [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) & `codec-d/cool_fox` — pre-grouped `TablePlan` (`46e3297`), batched/deduplicated card-copy barriers (`773d902`), and per-list pipeline bind deduplication (`bound.rs`).
- [voidopcode/vilocity](https://github.com/voidopcode/vilocity) — unused ray-tracing & ray-feeder pass skip patterns (`2681546144`).
- [KomanderDG88/VIniVIdiVIci](https://github.com/KomanderDG88/VIniVIdiVIci) — buffer-pass copy cleanup on descriptor heap drop (`82d246c`).
- [edsuuu/VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) — interval `CleanBlocks` (`ac6e475108`, based on `victory-fork#824`), per-frame `Uncommitted` view-block memo (`136efde0ab`), and `tippythetaidum`'s 16-bit `half_views` facial animation fixes (`189ee55a`, `a07a5869`, `221a2a09`, `22f3426d`).
- [kitty-core/Kitty6](https://github.com/kitty-core/Kitty6) (and ported upstream/community fixes from `YukiRyuu/RYUKI-7900XT`, `TaynanGT/Vice-Tay`, `Fayberr/Victory-United`, `trustno1-coder/VIctory-NVIDIA`, `Op-lite/victory-scarlett`, `Moon0755`, `RVZO6`, `shh`, `Henrique Bueno`, `welisonzika14-jpg`, `Indy2l`, `myokex`, `zocram4cc`) — Waves 5–6 lock fixes, root window reach & cache, memory pools, async `texback`, `Taken`/`Slots`/`Queued` lock-free bitsets, sharded `RETIRED`, `VICTORY_FILL_TITLE`, chunked unwatched diff uploads, early table release, BC7 mode 4/5 rotation, RDNA 2 `v_fmac_f32` modifiers, `ForceEarlyDepthStencil`, `VICTORY_D16_BITS`, `VICTORY_SPILL_STRICT`, aliased view tracking, and `half_views` integration.
- Upstream PR contributors: `0xKaji` ([VIctory#2752](https://github.com/Marvisak/VIctory/pull/2752)), [VIctory#2735](https://github.com/Marvisak/VIctory/pull/2735), `0xkn0wh3r3` ([VIctory#3011](https://github.com/Marvisak/VIctory/pull/3011)), `Luislariosss` ([VIctory#3025](https://github.com/Marvisak/VIctory/pull/3025)), `fuzeofficial` ([VIctory#3046](https://github.com/Marvisak/VIctory/pull/3046)), `pes7` ([VIctory#2634](https://github.com/Marvisak/VIctory/pull/2634)), `RVZO6` ([VIctory#3083](https://github.com/Marvisak/VIctory/pull/3083)), `pablogonz12` ([VIctory#3167](https://github.com/Marvisak/VIctory/pull/3167)), `spy0eye-beep` ([klbf-lab#23](https://github.com/PastiiJay/klbf-lab/pull/23)), & `Vladimir-Human` ([klbf-lab#16](https://github.com/PastiiJay/klbf-lab/pull/16)).

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE).
