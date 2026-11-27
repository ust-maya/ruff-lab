//! Adapted from Belobug/VIctory-Belobug-AMD (`4f62c2a`, GPL-3.0-or-later).
//!
//! A least amount of indirect light that follows the sky dome by default.
//!
//! Grand Theft Auto VI's indirect diffuse light comes from probes it fills
//! by tracing rays, which do not run here yet: its program that resolves
//! them (`CS_ScreenProbes_TemporalResolve_*`) leaves next to nothing, and
//! whatever the sun does not reach directly is black. Right after that
//! program, its output is replaced by sky-dome-following ambient light
//! (`VICTORY_AMBIENT_SKY=off` keeps a constant `VICTORY_AMBIENT`, default `0.6`;
//! `VICTORY_AMBIENT=off` disables the pass completely).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

use crate::host::{HostList, host};
use crate::object::This;

/// The programs it follows, by the start of their name.
pub(crate) const PROGRAM: &[u8] = b"CS_ScreenProbes_TemporalResolve";
/// Where they write the indirect diffuse light (R9G9B9E5 as 32-bit
/// integers): root parameter and place in its table.
pub(crate) const LIGHT: (u8, u32) = (1, 176);

/// 0.6 looked balanced in run a2. `VICTORY_AMBIENT=off` leaves what the
/// title's resolve wrote; 0 is no indirect diffuse light at all.
const DEFAULT: f32 = 0.6;

/// The floor; `None` if off.
pub(crate) fn level() -> Option<f32> {
    static LEVEL: OnceLock<f32> = OnceLock::new();
    let level = *LEVEL.get_or_init(|| {
        let set = std::env::var("VICTORY_AMBIENT").unwrap_or_default();
        if set.trim() == "off" {
            return -1.0;
        }
        set.trim().parse().ok().filter(|l: &f32| l.is_finite() && *l >= 0.0).unwrap_or(DEFAULT)
    });
    (level >= 0.0).then_some(level)
}

/// The light's colour: warm white, or what `VICTORY_AMBIENT_TINT=r,g,b` gives.
fn tint() -> [f32; 3] {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, tint)) = *live {
        return tint;
    }
    static SET: OnceLock<[f32; 3]> = OnceLock::new();
    *SET.get_or_init(|| {
        let set: Vec<f32> = std::env::var("VICTORY_AMBIENT_TINT")
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        match set[..] {
            [r, g, b] => [r, g, b],
            _ => [1.0, 0.98, 0.94],
        }
    })
}

/// The level and colour last read from `VICTORY_AMBIENT_FILE`, if any.
static LIVE: std::sync::Mutex<Option<(f32, [f32; 3])>> = std::sync::Mutex::new(None);

/// The program that writes the title's reflections for the last time in a
/// frame, by the start of its name, and where it writes them.
pub(crate) const REFLECTED: &[u8] = b"CS_UpdateHistory";
pub(crate) const REFLECTIONS: (u8, u32) = (1, 40);

/// The most a reflection may hold, as a part of the ambient light's level:
/// `VICTORY_AMBIENT_REFLECT=<part>` (0.5 by default; 0 leaves reflections alone).
static KEEP: std::sync::Mutex<Option<f32>> = std::sync::Mutex::new(None);

/// The least a reflection holds, as a part of the light around (0 by default).
static LEAST: std::sync::Mutex<f32> = std::sync::Mutex::new(0.0);

fn keep() -> f32 {
    if let Some(live) = *KEEP.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| {
        let default = if crate::host::low_graphics() > 0 { 0.02 } else { 0.5 };
        std::env::var("VICTORY_AMBIENT_REFLECT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|k: &f32| k.is_finite() && *k >= 0.0)
            .unwrap_or(default)
    })
}

/// The program that draws the sky as the title reflects it, by the start of
/// its name: a small image of the whole sky, in the light's own units.
pub(crate) const SKY: &[u8] = b"ps_sky_paraboloid_reflection";

/// The sky dome that program last drew into: the host copy, held, since
/// the title's object may go at any time.
static SKY_IMAGE: std::sync::Mutex<Option<std::sync::Arc<crate::resource::HostTexture>>> =
    std::sync::Mutex::new(None);

pub(crate) fn sky_drawn(image: usize) {
    let Some(copy) = crate::resource::host_copy(image as This) else { return };
    // SAFETY: a live resource.
    let d = unsafe { copy.texture.GetDesc() };
    if d.Format != DXGI_FORMAT_R16G16B16A16_FLOAT || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        return;
    }
    let mut held = SKY_IMAGE.lock().unwrap_or_else(|e| e.into_inner());
    if !held.as_ref().is_some_and(|h| std::sync::Arc::ptr_eq(h, &copy)) {
        *held = Some(copy);
    }
}

/// Default sky-following curve (`[gain, least, power]`): drops smoothly to
/// ~0.10-0.13 in dark/night scenes (preventing white auto-exposure blowout on
/// characters and cars), keeps dawn/indoor scenes ~0.18-0.30 (avoiding black
/// silhouettes), and caps at `level` (0.6) in bright daylight.
const DEFAULT_SKY: [f32; 3] = [0.44, 0.075, 0.36];

/// Conservative fallback level when sky-following is active before the sky
/// paraboloid has been drawn for the first time.
const SKY_FALLBACK_CAP: f32 = 0.28;

/// Sky-dome-following ambient (`VICTORY_AMBIENT_SKY`, ON by default with
/// `0.44,0.075,0.36`; `off` or `0` reverts to constant `VICTORY_AMBIENT`).
static FOLLOW: std::sync::Mutex<Option<Option<[f32; 3]>>> = std::sync::Mutex::new(None);

fn follow() -> Option<[f32; 3]> {
    if let Some(live) = *FOLLOW.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<Option<[f32; 3]>> = OnceLock::new();
    *SET.get_or_init(|| {
        let Ok(set) = std::env::var("VICTORY_AMBIENT_SKY") else {
            return Some(DEFAULT_SKY);
        };
        let trimmed = set.trim();
        if trimmed == "0" || trimmed.eq_ignore_ascii_case("off") {
            return None;
        }
        let mut follow = DEFAULT_SKY;
        if !trimmed.is_empty() && trimmed != "1" && !trimmed.eq_ignore_ascii_case("on") {
            for (to, v) in follow.iter_mut().zip(trimmed.split(',')) {
                if let Ok(v) = v.trim().parse() {
                    *to = v;
                }
            }
        }
        Some(follow)
    })
}

/// CPU mirror of the HLSL sky-following brightness curve (used by unit tests).
#[cfg(test)]
fn sky_amount(mean_luma: f32, [gain, least, power]: [f32; 3], cap: f32) -> f32 {
    (gain * mean_luma.max(0.0).powf(power) + least).min(cap.max(least))
}

/// Where the resolve reads the scene's depth (reversed, D32 with stencil):
/// root parameter and place in its table.
pub(crate) const DEPTH: (u8, u32) = (0, 464);

/// Depth-based crease occlusion: `VICTORY_AMBIENT_SHADE=s,r,n,t` (0 disables).
static SHADE: std::sync::Mutex<Option<[f32; 4]>> = std::sync::Mutex::new(None);

fn shade() -> [f32; 4] {
    if let Some(live) = *SHADE.lock().unwrap_or_else(|e| e.into_inner()) {
        return live;
    }
    static SET: OnceLock<[f32; 4]> = OnceLock::new();
    *SET.get_or_init(|| {
        let mut shade = if crate::host::low_graphics() > 0 {
            [0.0, 0.6, 0.1, 0.45]
        } else {
            [1.0, 0.6, 0.1, 0.45]
        };
        let set = std::env::var("VICTORY_AMBIENT_SHADE").unwrap_or_default();
        for (to, v) in shade.iter_mut().zip(set.split(',')) {
            if let Ok(v) = v.trim().parse() {
                *to = v;
            }
        }
        shade
    })
}

/// The level from `VICTORY_AMBIENT_FILE`, if it gave one; starts the thread
/// that reads the file (diagnostics builds).
fn live_level() -> Option<f32> {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        if !cfg!(feature = "diagnostics") {
            return;
        }
        let Some(file) = std::env::var_os("VICTORY_AMBIENT_FILE") else { return };
        let _ = std::thread::Builder::new().name("victory-ambient".into()).spawn(move || {
            loop {
                let read: Vec<f32> = std::fs::read_to_string(&file)
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if let [gain, least, ref rest @ ..] = read[..]
                    && matches!(rest.len(), 0 | 1 | 4 | 5)
                {
                    let (power, rest) = match rest.len() % 2 {
                        1 => (rest[0], &rest[1..]),
                        _ => (0.4, rest),
                    };
                    if let [s, r, n, t] = rest[..] {
                        *SHADE.lock().unwrap_or_else(|e| e.into_inner()) = Some([s, r, n, t]);
                    }
                    let new = Some(Some([gain, least, power]));
                    let mut follow = FOLLOW.lock().unwrap_or_else(|e| e.into_inner());
                    if *follow != new {
                        tracing::info!(gain, least, power, "ambient light follows the sky, set from the file");
                        *follow = new;
                    }
                } else if let [level, r, g, b, ref rest @ ..] = read[..] {
                    *FOLLOW.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
                    if let [s, r, n, t, ref last @ ..] = rest[..] {
                        *SHADE.lock().unwrap_or_else(|e| e.into_inner()) = Some([s, r, n, t]);
                        if let [k, ref floor @ ..] = last[..] {
                            *KEEP.lock().unwrap_or_else(|e| e.into_inner()) = Some(k.max(0.0));
                            *LEAST.lock().unwrap_or_else(|e| e.into_inner()) =
                                floor.first().map_or(0.0, |f| f.max(0.0));
                        }
                    }
                    let new = Some((level.max(0.0), [r, g, b]));
                    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
                    if *live != new {
                        tracing::info!(level, r, g, b, "ambient light set from the file");
                        *live = new;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    });
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).map(|(level, _)| level)
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=13, b0), DescriptorTable(UAV(u0), SRV(t0), SRV(t1))"
cbuffer Floor : register(b0) {
    float level; float3 tint;
    float strength; float radius; float near; float tan_y;
    float gain; float least;
    float held; float floor_part; float power;
};
Texture2DArray<float4> sky : register(t1);
float3 from_around() {
    if (gain <= 0.0) return tint * level;
    uint sw, sh, slices;
    sky.GetDimensions(sw, sh, slices);
    if (sw == 0 || sh == 0) return tint * min(level, 0.28);
    float3 sum = 0.0;
    const int N = 8;
    for (int y = 0; y < N; y++)
        for (int x = 0; x < N; x++) {
            float3 there = sky.Load(int4((x * 2 + 1) * sw / (N * 2), (y * 2 + 1) * sh / (N * 2), 0, 0)).rgb;
            sum += clamp(there, 0.0, 8.0);
        }
    float3 mean = sum / (N * N);
    const float3 LUMA = float3(0.2126, 0.7152, 0.0722);
    float m = dot(mean, LUMA);
    // Unit-luma cool night tint so low-light interiors keep their calibrated brightness.
    const float3 NIGHT = float3(0.80, 0.96, 1.24);
    float3 sky_hue = mean / max(m, 1e-6);
    float3 raw_hue = lerp(NIGHT, lerp(float3(1.0, 1.0, 1.0), sky_hue, 0.45), saturate(m / 0.04));
    float3 hue = raw_hue / max(dot(raw_hue, LUMA), 1e-4);
    float amount = min(gain * pow(max(m, 0.0), power) + least, max(level, least));
    return hue * amount;
}
RWTexture2D<uint> light : register(u0);
Texture2D<float> depth : register(t0);
float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = near / max(d, 1e-7);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}
float open_to_light(int2 p, float2 size) {
    float3 c = seen(p, size);
    if (c.z > 400.0) return 1.0;
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx));
    if (dot(n, c) > 0.0) n = -n;
    float px = radius * size.y / (2.0 * tan_y * c.z);
    if (px < 1.5) return 1.0;
    px = min(px, 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    float sum = 0.0;
    const int N = 16;
    for (int i = 0; i < N; i++) {
        float a = i * 2.399963 + noise * 6.2831853;
        float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
        float3 v = seen(p + int2(round(o)), size) - c;
        float vv = dot(v, v);
        float over = dot(v, n) - 0.004 * c.z - 0.005;
        sum += max(over, 0.0) / sqrt(vv + 1e-6) * saturate(1.0 - vv / (radius * radius));
    }
    return saturate(1.0 - strength * 2.0 * sum / N);
}
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float most = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(most, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (most * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    light.GetDimensions(w, h);
    if (at.x >= w || at.y >= h) return;
    if (held > 0.0) {
        const float3 LUMA = float3(0.2126, 0.7152, 0.0722);
        float3 seen_there = unpack(light[at.xy]);
        float most = dot(from_around(), LUMA) * held;
        float has = dot(seen_there, LUMA);
        float3 floor_light = from_around() * floor_part;
        if (has > most) light[at.xy] = pack(seen_there * (most / has));
        else if (has < dot(floor_light, LUMA)) light[at.xy] = pack(floor_light);
        return;
    }
    float open = strength > 0.0 ? open_to_light(int2(at.xy), float2(w, h)) : 1.0;
    light[at.xy] = pack(from_around() * open);
}
"#;

/// Views, used in turn: more than the passes of every list in flight.
const SLOTS: u32 = 1024;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn make(device: &ID3D12Device) -> windows::core::Result<Pass> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("ambient"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    let text = |b: &ID3DBlob| {
        // SAFETY: the blob's own bytes.
        unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
            .to_vec()
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            tracing::warn!("ambient light: {}", String::from_utf8_lossy(&text(&errors)));
        }
        return Err(e);
    }
    let code = text(&code.ok_or_else(windows::core::Error::empty)?);
    // SAFETY: the root signature is inside the shader; plain creations.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: code.as_ptr().cast(),
                BytecodeLength: code.len(),
            },
            ..Default::default()
        })?;
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: SLOTS * 3,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        Ok(Pass {
            root,
            pso,
            views,
            step: device.GetDescriptorHandleIncrementSize(kind),
            next: AtomicU32::new(0),
        })
    }
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        ..Default::default()
    };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// Raises what `light` holds to the floor, in `list`, after the program that
/// wrote it as an image.
pub(crate) fn raise(list: &HostList, light: This, depth: Option<This>) -> bool {
    record(list, light, depth, 0.0)
}

/// Holds the reflections in `image` to a part of the ambient light, in `list`.
pub(crate) fn hold(list: &HostList, image: This) -> bool {
    let held = keep();
    if held <= 0.0 || !record(list, image, None, held) {
        return false;
    }
    static HELD: std::sync::Once = std::sync::Once::new();
    HELD.call_once(|| tracing::info!(held, "reflections held to the ambient light"));
    true
}

fn record(list: &HostList, light: This, depth: Option<This>, held: f32) -> bool {
    let (Some(level), Some(host)) = (level(), host()) else {
        return false;
    };
    let Some(light) = crate::resource::host_resource(light) else {
        return false;
    };
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    let Some(pass) = PASS
        .get_or_init(|| {
            make(&host.device)
                .inspect_err(|e| tracing::warn!(error = %e, "no ambient light"))
                .ok()
        })
        .as_ref()
    else {
        return false;
    };
    // SAFETY: a live resource.
    let d = unsafe { light.GetDesc() };
    static ONCE: std::sync::Once = std::sync::Once::new();
    if d.Format != DXGI_FORMAT_R32_UINT {
        ONCE.call_once(|| tracing::warn!(format = d.Format.0, "no ambient light: another format"));
        return false;
    }
    ONCE.call_once(|| {
        tracing::info!(level, width = d.Width, height = d.Height, "ambient light");
    });
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SLOTS * 3;
    let sky_image = SKY_IMAGE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let sky = follow().and_then(|f| {
        let t = &sky_image.as_ref()?.texture;
        // SAFETY: a live resource.
        let d = unsafe { t.GetDesc() };
        static SEEN: std::sync::Once = std::sync::Once::new();
        SEEN.call_once(|| {
            tracing::info!(
                format = d.Format.0,
                width = d.Width,
                height = d.Height,
                slices = d.DepthOrArraySize,
                "the sky image the ambient light would follow"
            );
        });
        Some((t, f, d.Format))
    });
    static FOLLOWS: std::sync::Once = std::sync::Once::new();
    if let Some((t, f, _)) = sky {
        FOLLOWS.call_once(|| {
            tracing::info!(gain = f[0], least = f[1], power = f[2], "ambient light follows the sky");
        });
        list.keep(t);
        let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0;
        if let Some(copy) = &sky_image
            && let Some(before) = list.transition_copy(copy, read as u32)
        {
            let barrier = crate::host::transition(
                t,
                D3D12_RESOURCE_STATES(before as i32),
                D3D12_RESOURCE_STATES(read),
            );
            // SAFETY: an open list and a live texture the list keeps.
            unsafe { list.list.ResourceBarrier(&[barrier]) };
        }
    }
    list.keep(light);
    let depth = depth.and_then(crate::resource::host_resource).filter(|t| {
        // SAFETY: a live resource.
        let t = unsafe { t.GetDesc() };
        t.Format == DXGI_FORMAT_R32G8X24_TYPELESS && (t.Width, t.Height) == (d.Width, d.Height)
    });
    if let Some(depth) = depth {
        list.keep(depth);
    }
    let l = &list.list;
    uav_barrier(l);
    // SAFETY: an open list; live textures kept by the list; dedicated descriptor slots.
    unsafe {
        let cpu = pass.views.GetCPUDescriptorHandleForHeapStart();
        let gpu = pass.views.GetGPUDescriptorHandleForHeapStart();
        let at = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + (slot * pass.step) as usize,
        };
        let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        host.device
            .CreateUnorderedAccessView(light, None::<&ID3D12Resource>, Some(&view), at);
        let seen = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() },
            },
        };
        let beside = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: at.ptr + pass.step as usize };
        host.device.CreateShaderResourceView(depth, Some(&seen), beside);
        let around = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: sky.map_or(DXGI_FORMAT_R16G16B16A16_FLOAT, |s| s.2),
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2DARRAY,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2DArray: D3D12_TEX2D_ARRAY_SRV {
                    MipLevels: 1,
                    ArraySize: 1,
                    ..Default::default()
                },
            },
        };
        let third = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: at.ptr + 2 * pass.step as usize };
        host.device.CreateShaderResourceView(sky.map(|s| s.0), Some(&around), third);
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(&pass.root);
        l.SetPipelineState(&pass.pso);
        let [r, g, b] = tint();
        let base_level = live_level().unwrap_or(level);
        // When sky-following is active before the sky dome has been drawn,
        // avoid flashing full daytime `0.6` in dark opening/night scenes.
        let effective_level = if follow().is_some() && sky.is_none() {
            base_level.min(SKY_FALLBACK_CAP)
        } else {
            base_level
        };
        let [strength, radius, near, tan_y] = shade();
        let strength = if depth.is_some() { strength } else { 0.0 };
        let [gain, least, power] = sky.map_or([0.0, 0.0, 1.0], |s| s.1);
        let floor = *LEAST.lock().unwrap_or_else(|e| e.into_inner());
        let constants = [effective_level, r, g, b, strength, radius, near, tan_y, gain, least, held, floor, power];
        l.SetComputeRoot32BitConstants(0, 13, constants.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(
            1,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + u64::from(slot * pass.step),
            },
        );
        l.Dispatch((d.Width as u32).div_ceil(8), d.Height.div_ceil(8), 1);
    }
    uav_barrier(l);
    true
}

#[cfg(test)]
mod tests {
    /// The pass compiles with the system's compiler and the host takes it.
    #[test]
    fn ambient_pass_builds() {
        let Some(host) = crate::host::host() else {
            return;
        };
        super::make(&host.device).expect("the ambient pass");
    }

    #[test]
    fn sky_curve_tames_night_blowout_and_caps_daylight() {
        let night = super::sky_amount(0.0005, super::DEFAULT_SKY, super::DEFAULT);
        let dusk_cell = super::sky_amount(0.02, super::DEFAULT_SKY, super::DEFAULT);
        let noon = super::sky_amount(3.0, super::DEFAULT_SKY, super::DEFAULT);
        assert!((0.08..=0.13).contains(&night), "night={night}");
        assert!((0.16..=0.24).contains(&dusk_cell), "dusk_cell={dusk_cell}");
        assert!((noon - super::DEFAULT).abs() < 1e-6, "noon={noon}");
    }
}
