//! Native Sony DualShock 4 and DualSense HID gamepad support (based on
//! `realblastex/flamingo`'s `dualsense.rs`, extended to DualShock 4 v1/v2).
//!
//! Acts as the title's first gamepad when no XInput pad is connected, so
//! DualShock 4 / DualSense controllers work out of the box over USB or
//! Bluetooth without x360ce, ViGEmBus, or DS4Windows. Buttons map to Xbox
//! positions (Cross -> A, Circle -> B, Square -> X, Triangle -> Y; Options ->
//! Menu; Share/Create and touchpad click -> View). Set `VICTORY_SONY_PAD=0`
//! to disable.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, RIGHT, RIGHT_SHOULDER, RIGHT_STICK, UP,
    VIEW, X, Y,
};

/// One report as the title's pad sees it: buttons in the title's bits,
/// triggers 0..1, sticks -1..1 (up and right positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Report {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    DualShock4,
    DualSense,
}

/// Sony Interactive Entertainment vendor ID.
const VENDOR: u16 = 0x054C;

/// Recognised Sony controller product IDs.
fn kind_of(product_id: u16) -> Option<Kind> {
    match product_id {
        0x05C4 | 0x09CC | 0x0BA0 => Some(Kind::DualShock4),
        0x0CE6 | 0x0DF2 => Some(Kind::DualSense),
        _ => None,
    }
}

/// A report older than this means the controller went away.
const STALE: Duration = Duration::from_secs(2);

static LAST: Mutex<Option<(Report, Instant)>> = Mutex::new(None);

/// The last report, if one came lately. Starts the reader thread on first use.
pub(super) fn latest() -> Option<Report> {
    static START: std::sync::Once = std::sync::Once::new();
    if std::env::var_os("VICTORY_SONY_PAD").is_some_and(|v| v == "0") {
        return None;
    }
    START.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("victory-sony-pad".into())
            .spawn(read_forever);
    });
    let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    last.and_then(|(r, at)| (at.elapsed() < STALE).then_some(r))
}

/// Parse a DualShock 4 or DualSense HID input report into Xbox pad state.
///
/// Layouts:
/// - DualShock 4 USB `0x01` (or short BT `0x01`): sticks 1..5, buttons 5..8, triggers 8..10.
/// - DualShock 4 full Bluetooth `0x11`: sticks 3..7, buttons 7..10, triggers 10..12.
/// - DualSense USB `0x01`: sticks 1..5, triggers 5..7, buttons 8..11.
/// - DualSense full Bluetooth `0x31`: sticks 2..6, triggers 6..8, buttons 9..12.
/// - DualSense short Bluetooth `0x01`: sticks 1..5, buttons 5..8, triggers 8..10.
pub(super) fn parse(b: &[u8], usb: bool, kind: Kind) -> Option<Report> {
    let (sticks, triggers, buttons) = match (kind, *b.first()?) {
        (Kind::DualShock4, 0x01) if b.len() >= 10 => (1, 8, 5),
        (Kind::DualShock4, 0x11) if b.len() >= 12 => (3, 10, 7),
        (Kind::DualSense, 0x01) if usb && b.len() >= 11 => (1, 5, 8),
        (Kind::DualSense, 0x31) if b.len() >= 12 => (2, 6, 9),
        (Kind::DualSense, 0x01) if b.len() >= 10 => (1, 8, 5),
        _ => return None,
    };
    let axis = |v: u8| ((f32::from(v) - 128.0) / 127.0).clamp(-1.0, 1.0);
    let (b0, b1, b2) = (b[buttons], b[buttons + 1], b[buttons + 2]);
    let mut bits = 0;
    // Hat: 0 up, clockwise in eighths; 8 neutral.
    let hat = b0 & 0x0F;
    if matches!(hat, 7 | 0 | 1) {
        bits |= UP;
    }
    if matches!(hat, 1..=3) {
        bits |= RIGHT;
    }
    if matches!(hat, 3..=5) {
        bits |= DOWN;
    }
    if matches!(hat, 5..=7) {
        bits |= LEFT;
    }
    for (on, bit) in [
        (b0 & 0x10, X),
        (b0 & 0x20, A),
        (b0 & 0x40, B),
        (b0 & 0x80, Y),
        (b1 & 0x01, LEFT_SHOULDER),
        (b1 & 0x02, RIGHT_SHOULDER),
        (b1 & 0x10, VIEW),
        (b1 & 0x20, MENU),
        (b1 & 0x40, LEFT_STICK),
        (b1 & 0x80, RIGHT_STICK),
        (b2 & 0x02, VIEW),
    ] {
        if on != 0 {
            bits |= bit;
        }
    }
    Some(Report {
        buttons: bits,
        left_trigger: f32::from(b[triggers]) / 255.0,
        right_trigger: f32::from(b[triggers + 1]) / 255.0,
        left_x: axis(b[sticks]),
        left_y: -axis(b[sticks + 1]),
        right_x: axis(b[sticks + 2]),
        right_y: -axis(b[sticks + 3]),
    })
}

/// Finds a Sony controller, reads it until it disconnects, then looks again.
fn read_forever() {
    loop {
        match open() {
            Some((handle, kind)) => {
                read(handle, kind);
                // SAFETY: handle returned by `CreateFileW` in `open`.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            None => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// Opens the first connected DualShock 4 or DualSense HID device.
fn open() -> Option<(windows_sys::Win32::Foundation::HANDLE, Kind)> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    // SAFETY: standard Win32 SetupAPI/HID enumeration with properly sized buffers;
    // every handle and device info set is closed or returned.
    unsafe {
        let mut guid = std::mem::zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(
            &guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if set == -1 {
            return None;
        }
        let mut found = None;
        for index in 0.. {
            let mut data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, index, &mut data) == 0 {
                break;
            }
            let mut detail = [0u32; 256];
            let d = detail
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*d).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            let size = size_of_val(&detail) as u32;
            if SetupDiGetDeviceInterfaceDetailW(
                set,
                &data,
                d,
                size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                continue;
            }
            let path = (&raw const (*d).DevicePath).cast::<u16>();
            let handle = CreateFileW(
                path,
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut attributes: HIDD_ATTRIBUTES = std::mem::zeroed();
            attributes.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
            if HidD_GetAttributes(handle, &mut attributes)
                && attributes.VendorID == VENDOR
                && let Some(kind) = kind_of(attributes.ProductID)
            {
                let mut feature = [0u8; 64];
                feature[0] = match kind {
                    Kind::DualShock4 => 0x02,
                    Kind::DualSense => 0x05,
                };
                let _ = HidD_GetFeature(
                    handle,
                    feature.as_mut_ptr().cast(),
                    feature.len() as u32,
                );
                tracing::info!(
                    ?kind,
                    product = format_args!("{:#06x}", attributes.ProductID),
                    "Sony HID gamepad found: bound as slot 0 while no XInput pad is connected"
                );
                found = Some((handle, kind));
                break;
            }
            CloseHandle(handle);
        }
        SetupDiDestroyDeviceInfoList(set);
        found
    }
}

/// Builds a DualShock 4 USB HID output report (32 bytes) setting the lightbar RGB.
fn ds4_usb_lightbar(r: u8, g: u8, b: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = 0x05; // USB output report ID
    out[1] = 0x07; // enable rumble | lightbar | flash
    out[6] = r;
    out[7] = g;
    out[8] = b;
    out
}

/// IEEE 802.3 CRC-32 used by DualShock 4 Bluetooth HID output reports.
fn crc32_ieee(prefix: u8, data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in std::iter::once(prefix).chain(data.iter().copied()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a DualShock 4 Bluetooth HID output report (78 bytes) setting the lightbar RGB.
fn ds4_bt_lightbar(r: u8, g: u8, b: u8) -> [u8; 78] {
    let mut out = [0u8; 78];
    out[0] = 0x11; // Bluetooth output report ID
    out[1] = 0xC0; // HID + poll rate
    out[2] = 0x20;
    out[3] = 0x07; // enable rumble | lightbar | flash
    out[8] = r;
    out[9] = g;
    out[10] = b;
    let crc = crc32_ieee(0xA2, &out[..74]);
    out[74..78].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Builds a DualSense USB HID output report (48 bytes) setting the lightbar RGB.
fn dualsense_usb_lightbar(r: u8, g: u8, b: u8) -> [u8; 48] {
    let mut out = [0u8; 48];
    out[0] = 0x02; // USB output report ID
    out[2] = 0x04; // lightbar color valid
    out[45] = r;
    out[46] = g;
    out[47] = b;
    out
}

/// Lights up the controller's lightbar so the user can see it is bound and active.
fn set_lightbar(
    handle: windows_sys::Win32::Foundation::HANDLE,
    kind: Kind,
    usb: bool,
    r: u8,
    g: u8,
    b: u8,
) {
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    let report_usb_ds4;
    let report_bt_ds4;
    let report_usb_ds5;
    let bytes: &[u8] = match (kind, usb) {
        (Kind::DualShock4, true) => {
            report_usb_ds4 = ds4_usb_lightbar(r, g, b);
            &report_usb_ds4
        }
        (Kind::DualShock4, false) => {
            report_bt_ds4 = ds4_bt_lightbar(r, g, b);
            &report_bt_ds4
        }
        (Kind::DualSense, true) => {
            report_usb_ds5 = dualsense_usb_lightbar(r, g, b);
            &report_usb_ds5
        }
        (Kind::DualSense, false) => return,
    };
    let mut written = 0u32;
    // SAFETY: valid handle opened with GENERIC_WRITE and valid report slice.
    let ok = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        tracing::info!(?kind, usb, r, g, b, "Sony HID gamepad lightbar set");
    }
}

/// Reads HID input reports until the controller disconnects.
fn read(handle: windows_sys::Win32::Foundation::HANDLE, kind: Kind) {
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    let mut buffer = [0u8; 128];
    let mut lit = false;
    loop {
        let mut n = 0u32;
        // SAFETY: valid handle and writable buffer of `buffer.len()` bytes.
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || n == 0 {
            return;
        }
        let usb = n <= 64;
        if let Some(report) = parse(&buffer[..n as usize], usb, kind) {
            if !lit {
                lit = true;
                set_lightbar(handle, kind, usb, 0, 220, 90);
            }
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((report, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ds4_usb_report() {
        let mut b = [0u8; 64];
        b[0] = 0x01;
        b[1..5].copy_from_slice(&[255, 0, 128, 128]);
        b[5] = 0x20 | 1; // Cross (A) + hat up-right
        b[6] = 0x20 | 0x01; // Options (Menu) + L1
        b[7] = 0x02; // Touchpad click (View)
        b[8] = 255; // L2 full
        b[9] = 0; // R2 rest
        let r = parse(&b, true, Kind::DualShock4).unwrap();
        assert_eq!(r.buttons, A | UP | RIGHT | MENU | LEFT_SHOULDER | VIEW);
        assert_eq!((r.left_x, r.left_y), (1.0, 1.0));
        assert!(r.right_x.abs() < 0.01 && r.right_y.abs() < 0.01);
        assert_eq!((r.left_trigger, r.right_trigger), (1.0, 0.0));
    }

    #[test]
    fn ds4_bluetooth_report() {
        let mut b = [0u8; 78];
        b[0] = 0x11;
        b[3..7].copy_from_slice(&[0, 255, 128, 128]);
        b[7] = 0x40 | 8; // Circle (B), hat neutral
        b[11] = 255; // R2 full
        let r = parse(&b, false, Kind::DualShock4).unwrap();
        assert_eq!(r.buttons, B);
        assert_eq!((r.left_x, r.left_y), (-1.0, -1.0));
        assert_eq!((r.left_trigger, r.right_trigger), (0.0, 1.0));
    }

    #[test]
    fn dualsense_usb_and_bt_reports() {
        let mut u = [0u8; 64];
        u[0] = 0x01;
        u[1..5].copy_from_slice(&[255, 0, 128, 128]);
        u[5] = 255;
        u[8] = 0x20 | 8; // Cross (A)
        let r = parse(&u, true, Kind::DualSense).unwrap();
        assert_eq!(r.buttons, A);
        assert_eq!((r.left_trigger, r.right_trigger), (1.0, 0.0));

        let mut bt = [0u8; 78];
        bt[0] = 0x31;
        bt[2..6].copy_from_slice(&[0, 255, 128, 128]);
        bt[9] = 0x80 | 8; // Triangle (Y)
        let r = parse(&bt, false, Kind::DualSense).unwrap();
        assert_eq!(r.buttons, Y);
        assert_eq!((r.left_x, r.left_y), (-1.0, -1.0));
        assert_eq!(parse(&[0x02, 0, 0], true, Kind::DualSense), None);
    }

    #[test]
    fn ds4_lightbar_reports_have_expected_headers_and_crc() {
        let usb = ds4_usb_lightbar(0, 220, 90);
        assert_eq!((usb[0], usb[1], usb[6], usb[7], usb[8]), (0x05, 0x07, 0, 220, 90));
        let bt = ds4_bt_lightbar(0, 220, 90);
        assert_eq!((bt[0], bt[1], bt[3], bt[8], bt[9], bt[10]), (0x11, 0xC0, 0x07, 0, 220, 90));
        let crc = u32::from_le_bytes(bt[74..78].try_into().unwrap());
        assert_eq!(crc, crc32_ieee(0xA2, &bt[..74]));
    }
}
