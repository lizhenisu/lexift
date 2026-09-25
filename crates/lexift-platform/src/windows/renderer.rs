//! Selects the startup renderer from the active Windows display configuration.

use windows::Win32::{
    Graphics::Gdi::{DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, EnumDisplayDevicesW},
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS},
};

pub(crate) fn prefer_software_renderer() -> bool {
    if unsafe { GetSystemMetrics(SM_CMONITORS) } != 1 {
        return false;
    }

    let mut index = 0;
    loop {
        let mut device = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        if !unsafe {
            EnumDisplayDevicesW(windows::core::PCWSTR::null(), index, &mut device, 0).as_bool()
        } {
            return false;
        }
        if device.StateFlags.0 & DISPLAY_DEVICE_PRIMARY_DEVICE.0 != 0 {
            return is_intel_display(&device.DeviceID, &device.DeviceString);
        }
        index += 1;
    }
}

fn is_intel_display(device_id: &[u16], name: &[u16]) -> bool {
    let id = String::from_utf16_lossy(device_id)
        .trim_end_matches('\0')
        .to_ascii_uppercase();
    let name = String::from_utf16_lossy(name)
        .trim_end_matches('\0')
        .to_ascii_lowercase();
    id.contains("VEN_8086") || name.contains("intel")
}

#[cfg(test)]
mod tests {
    use super::is_intel_display;

    #[test]
    fn primary_adapter_identity_detects_intel_without_matching_other_vendors() {
        assert!(is_intel_display(
            &"PCI\\VEN_8086&DEV_46A6".encode_utf16().collect::<Vec<_>>(),
            &[]
        ));
        assert!(is_intel_display(
            &[],
            &"Intel(R) Iris Xe".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(!is_intel_display(
            &"PCI\\VEN_10DE".encode_utf16().collect::<Vec<_>>(),
            &"NVIDIA GeForce".encode_utf16().collect::<Vec<_>>()
        ));
    }
}
