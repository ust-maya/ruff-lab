//! Copy queue for dirty card-buffer pages.
//!
//! Draws stay on the direct queue. Dirty pages move on this one. The copy
//! waits for a signal on the direct queue, so it starts after draws already
//! queued, and the direct queue waits for the copy fence before the next
//! hash and the next draws. The CPU waits for neither.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::{Host, Prelude, transition};
use crate::loopcut::{self, RING_ALIGN, RING_BYTES};
use crate::shadow::Shadow;

/// Non-zero while this submit is between the copy and the hash that reads it.
static IN_SUBMIT: AtomicU32 = AtomicU32::new(0);

struct Ring {
    buffer: ID3D12Resource,
    ptr: usize,
    cap: u64,
    cursor: u64,
    ticket: u64,
}

pub(super) struct Engine {
    rings: [Ring; 2],
    kick: Prelude,
    kick_ticket: u64,
    offload: Prelude,
    offload_ticket: u64,
}

pub(super) struct Hold {
    pub(super) resources: Vec<ID3D12Resource>,
    pub(super) lists: Vec<Prelude>,
}

impl Hold {
    pub(super) fn empty() -> Self {
        Self {
            resources: Vec::new(),
            lists: Vec::new(),
        }
    }
}

struct Op {
    dest: ID3D12Resource,
    dst: u64,
    src: ID3D12Resource,
    src_at: u64,
    len: u64,
}

pub(super) fn probe(device: &ID3D12Device) -> (bool, u64) {
    for bytes in [RING_BYTES, 1 << 20, 64 << 10] {
        if rebar_buffer(device, bytes).is_some() {
            return (true, bytes);
        }
    }
    (false, 0)
}

fn rebar_buffer(device: &ID3D12Device, size: u64) -> Option<(ID3D12Resource, *mut u8)> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_CUSTOM,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE,
        MemoryPoolPreference: D3D12_MEMORY_POOL_L1,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(1),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: a private allocation, mapped while the resource lives.
    let ok = unsafe {
        device
            .CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut buffer,
            )
            .is_ok()
            && buffer.as_ref().is_some_and(|b| b.Map(0, None, Some(&mut data)).is_ok())
    };
    ok.then(|| (buffer.unwrap(), data.cast()))
}

fn make_copy_list(device: &ID3D12Device) -> Option<Prelude> {
    // SAFETY: a copy allocator and a closed copy list.
    unsafe {
        let allocator = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
            .ok()?;
        let list: ID3D12GraphicsCommandList = device
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)
            .ok()?;
        list.Close().ok()?;
        Some((allocator, list))
    }
}

fn ensure(host: &Host, slot: &mut Option<Engine>) -> bool {
    if slot.is_some() {
        return true;
    }
    let device = &host.device;
    let Some(kick) = make_copy_list(device) else {
        return false;
    };
    let Some(offload) = make_copy_list(device) else {
        return false;
    };
    let mk = |i: usize| -> Option<Ring> {
        let made = if loopcut::use_rebar(RING_BYTES) {
            rebar_buffer(device, RING_BYTES).or_else(|| host.upload_buffer(RING_BYTES))
        } else {
            host.upload_buffer(RING_BYTES)
        }?;
        super::name(&made.0, || format!("dirty-page upload ring {i}"));
        Some(Ring {
            buffer: made.0,
            ptr: made.1 as usize,
            cap: RING_BYTES,
            cursor: 0,
            ticket: 0,
        })
    };
    let (Some(a), Some(b)) = (mk(0), mk(1)) else {
        return false;
    };
    *slot = Some(Engine {
        rings: [a, b],
        kick,
        kick_ticket: 0,
        offload,
        offload_ticket: 0,
    });
    true
}

fn completed(host: &Host) -> u64 {
    let Some(fence) = &host.copy_fence else {
        return 0;
    };
    // SAFETY: the fence is ours.
    unsafe { fence.GetCompletedValue() }
}

pub(super) fn begin_submit() {
    IN_SUBMIT.store(1, Ordering::Release);
}

pub(super) fn end_window() {
    IN_SUBMIT.store(0, Ordering::Release);
}

/// The direct queue waits for every copy already signalled. No CPU wait.
pub(super) fn wait_on_direct(host: &Host) {
    let Some(fence) = &host.copy_fence else {
        return;
    };
    let n = host.copy_next.load(Ordering::Acquire);
    let Some(n) = loopcut::copy_wait(n) else {
        return;
    };
    // SAFETY: the queue and the fence are ours. The wait is on the GPU.
    unsafe {
        let _ = host.queue.Wait(fence, n);
    }
}

/// Queue offsets `[from, to)` now. While a submit is hashing, the bytes
/// stay dirty for that submit's offload instead.
pub(super) fn kick(host: &Host, shadow: &Shadow, from: usize, to: usize) {
    if from >= to || IN_SUBMIT.load(Ordering::Acquire) != 0 || !host.copy_offload_on() {
        return;
    }
    let mut slot = host.copy_engine.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure(host, &mut slot) {
        return;
    }
    let done = completed(host);
    let Some(eng) = slot.as_mut() else {
        return;
    };
    if eng.kick_ticket > done {
        return;
    }
    for i in finished(done, &eng.rings) {
        eng.rings[i].cursor = 0;
    }
    let mut hold = Hold::empty();
    let mut ops = Vec::new();
    if !place(host, eng, done, shadow, from, to, true, &mut ops, &mut hold) {
        return;
    }
    if !queue(host, eng, true, &ops, &mut hold) {
        return;
    }
    // The bytes are on the copy queue. A later submit must not send them again.
    shadow.unmark_card(from, to);
    loopcut::note_inflight(shadow.id(), from, to);
}

/// Sends dirty ranges that are not already on the copy queue. Ranges that
/// do not fit stay in `direct` for the direct queue. Resources the copy
/// queue still reads are returned in the hold.
pub(super) fn offload(
    host: &Host,
    dirty: &[(std::sync::Arc<Shadow>, Vec<(usize, usize)>)],
    direct: &mut [Vec<(usize, usize)>],
) -> Hold {
    let mut hold = Hold::empty();
    if !host.copy_offload_on() {
        return hold;
    }
    let mut slot = host.copy_engine.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure(host, &mut slot) {
        return hold;
    }
    let done = completed(host);
    let Some(eng) = slot.as_mut() else {
        return hold;
    };
    for i in finished(done, &eng.rings) {
        eng.rings[i].cursor = 0;
    }
    let mut ops = Vec::new();
    for (i, (shadow, ranges)) in dirty.iter().enumerate() {
        let mut left = Vec::new();
        let mut mine = Vec::new();
        let mark = (
            ops.len(),
            hold.resources.len(),
            eng.rings[0].cursor,
            eng.rings[1].cursor,
        );
        let mut failed = false;
        for &(from, to) in ranges {
            if loopcut::inflight_covers(shadow.id(), from, to) {
                continue;
            }
            if place(
                host, eng, done, shadow, from, to, false, &mut mine, &mut hold,
            ) {
                continue;
            }
            failed = true;
            break;
        }
        if failed {
            eng.rings[0].cursor = mark.2;
            eng.rings[1].cursor = mark.3;
            hold.resources.truncate(mark.1);
            left.extend(
                ranges
                    .iter()
                    .copied()
                    .filter(|&(a, b)| !loopcut::inflight_covers(shadow.id(), a, b)),
            );
        } else {
            ops.extend(mine);
            let _ = mark.0;
        }
        if let Some(slot) = direct.get_mut(i) {
            *slot = left;
        }
    }
    if !ops.is_empty() && !queue(host, eng, false, &ops, &mut hold) {
        for (slot, (shadow, ranges)) in direct.iter_mut().zip(dirty) {
            *slot = ranges
                .iter()
                .copied()
                .filter(|&(a, b)| !loopcut::inflight_covers(shadow.id(), a, b))
                .collect();
        }
    }
    // `slot` stays until the end of this function, so a kick cannot land
    // between the wait below and the hash list the caller executes next.
    wait_on_direct(host);
    hold
}

fn place(
    host: &Host,
    eng: &mut Engine,
    done: u64,
    shadow: &Shadow,
    from: usize,
    to: usize,
    ring_only: bool,
    ops: &mut Vec<Op>,
    hold: &mut Hold,
) -> bool {
    let len = (to - from) as u64;
    if len == 0 {
        return true;
    }
    // A finished ring, then the other if the first has no room. `pick_ring`
    // is the same rule the tests check: ticket <= completed.
    let mut tickets = [eng.rings[0].ticket, eng.rings[1].ticket];
    while let Some(i) = loopcut::pick_ring(done, &tickets) {
        tickets[i] = u64::MAX;
        let ring = &mut eng.rings[i];
        let Some((src_at, next)) = loopcut::pack_offset(ring.cursor, ring.cap, len, RING_ALIGN)
        else {
            continue;
        };
        // SAFETY: the ring mapping covers `src_at + len`, and the staging
        // pointer covers `from + len`. The ring is not reused until `ticket`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                shadow.staging_ptr().add(from),
                (ring.ptr as *mut u8).add(src_at as usize),
                to - from,
            );
        }
        ring.cursor = next;
        ops.push(Op {
            dest: shadow.buffer.clone(),
            dst: from as u64,
            src: ring.buffer.clone(),
            src_at,
            len,
        });
        return true;
    }
    if ring_only {
        return false;
    }
    let Some((buffer, ptr)) = host.upload_buffer(len) else {
        return false;
    };
    super::name(&buffer, || format!("dirty-page upload {len}"));
    // SAFETY: the upload is mapped for `len` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(shadow.staging_ptr().add(from), ptr, to - from);
    }
    ops.push(Op {
        dest: shadow.buffer.clone(),
        dst: from as u64,
        src: buffer.clone(),
        src_at: 0,
        len,
    });
    hold.resources.push(buffer);
    true
}

/// Indexes of rings whose reader has finished. `u64::MAX` is never a ticket.
fn finished(done: u64, rings: &[Ring]) -> Vec<usize> {
    let mut tickets: Vec<u64> = rings.iter().map(|ring| ring.ticket).collect();
    let mut out = Vec::new();
    while let Some(i) = loopcut::pick_ring(done, &tickets) {
        tickets[i] = u64::MAX;
        out.push(i);
    }
    out
}

fn queue(host: &Host, eng: &mut Engine, kick_path: bool, ops: &[Op], hold: &mut Hold) -> bool {
    if ops.is_empty() {
        return true;
    }
    let Some(copy_queue) = host.copy_queue.clone() else {
        return false;
    };
    let Some(copy_fence) = host.copy_fence.clone() else {
        return false;
    };
    let Some(hand_fence) = host.hand_fence.clone() else {
        return false;
    };
    let done = completed(host);
    let ticket = if kick_path {
        eng.kick_ticket
    } else {
        eng.offload_ticket
    };
    if kick_path && ticket > done {
        return false;
    }
    let mut temp_list = None;
    if !kick_path && ticket > done {
        temp_list = make_copy_list(&host.device);
        if temp_list.is_none() {
            return false;
        }
    }
    // Recording borrows the list. The borrow ends with this block, before
    // the engine's tickets are stored.
    let copy_cmd = {
        let list: &mut Prelude = if let Some(list) = temp_list.as_mut() {
            list
        } else if kick_path {
            &mut eng.kick
        } else {
            &mut eng.offload
        };
        let (allocator, cl) = list;
        // SAFETY: the copy list is idle or brand new, the resources outlive
        // the call, and the fences belong to this device.
        let closed = unsafe {
            (|| -> windows::core::Result<ID3D12CommandList> {
                allocator.Reset()?;
                cl.Reset(&*allocator, None)?;
                let mut seen: Vec<usize> = Vec::new();
                let mut into_copy = Vec::new();
                let mut back = Vec::new();
                for op in ops {
                    let raw = op.dest.as_raw() as usize;
                    if !seen.contains(&raw) {
                        seen.push(raw);
                        into_copy.push(transition(
                            &op.dest,
                            D3D12_RESOURCE_STATE_COMMON,
                            D3D12_RESOURCE_STATE_COPY_DEST,
                        ));
                        back.push(transition(
                            &op.dest,
                            D3D12_RESOURCE_STATE_COPY_DEST,
                            D3D12_RESOURCE_STATE_COMMON,
                        ));
                    }
                }
                if !into_copy.is_empty() {
                    cl.ResourceBarrier(&into_copy);
                }
                for op in ops {
                    cl.CopyBufferRegion(&op.dest, op.dst, &op.src, op.src_at, op.len);
                }
                if !back.is_empty() {
                    cl.ResourceBarrier(&back);
                }
                cl.Close()?;
                cl.cast()
            })()
        };
        match closed {
            Ok(cmd) => cmd,
            Err(_) => return false,
        }
    };
    // A one-shot list has to outlive the copy. The engine's own lists stay put.
    if let Some(prep) = temp_list {
        hold.lists.push(prep);
    }
    let n = host.copy_next.fetch_add(1, Ordering::AcqRel) + 1;
    // Buffers decay to COMMON when ExecuteCommandLists finishes, so this
    // queue never barriers out of a shader-resource state. The direct
    // signal only orders the copy after draws already queued.
    // SAFETY: queues and fences are ours. The list was just closed.
    let submitted = unsafe {
        if host.queue.Signal(&hand_fence, n).is_err() || copy_queue.Wait(&hand_fence, n).is_err() {
            false
        } else {
            copy_queue.ExecuteCommandLists(&[Some(copy_cmd)]);
            true
        }
    };
    if !submitted {
        // `n` was published. Complete it from the CPU or a later direct
        // Wait(n) never finishes.
        unsafe {
            let _ = copy_fence.Signal(n);
        }
        return false;
    }
    unsafe {
        if copy_queue.Signal(&copy_fence, n).is_err() {
            tracing::error!("copy queue did not signal its fence; signalling it again");
            let _ = copy_queue.Signal(&copy_fence, n);
        }
        let _ = host.queue.Wait(&copy_fence, n);
    }
    if kick_path {
        eng.kick_ticket = n;
    } else {
        eng.offload_ticket = n;
    }
    for ring in &mut eng.rings {
        if ring.cursor > 0 {
            ring.ticket = n;
        }
    }
    loopcut::note_ring(ops.iter().map(|op| op.len).sum());
    loopcut::note_copy_signal();
    true
}
