//! WGC can execute internal teardown work after the last apartment retires.
//! Close has no public completion fence for that work. Keep only its code image
//! mapped for the process; sessions, callbacks, devices and workers still retire.
use std::sync::OnceLock;
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
            GetModuleHandleExW, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
        },
    },
    core::{PCWSTR, Result, w},
};

/// Load from the system directory before any WGC activation, then pin the exact
/// image by address. One temporary LoadLibrary reference is always released,
/// including failure; the successful Windows pin lasts until process exit.
pub(super) fn keep_code_loaded() -> Result<()> {
    static READY: OnceLock<Result<()>> = OnceLock::new();
    READY
        .get_or_init(|| {
            struct Loaded(HMODULE);
            impl Drop for Loaded {
                fn drop(&mut self) {
                    let _ = unsafe { FreeLibrary(self.0) };
                }
            }
            let loaded = Loaded(unsafe {
                LoadLibraryExW(
                    w!("GraphicsCapture.dll"),
                    None,
                    LOAD_LIBRARY_SEARCH_SYSTEM32,
                )?
            });
            let mut pinned = HMODULE::default();
            unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                    PCWSTR(loaded.0.0.cast()),
                    &mut pinned,
                )?;
            }
            Ok(())
        })
        .clone()
}

#[test]
#[ignore = "actual WGC module/apartment retirement check; interactive Windows required"]
fn capture_code_mapping_survives_full_apartment_retirement() {
    use windows::Win32::System::{
        LibraryLoader::GetModuleHandleW,
        ProcessStatus::{GetModuleInformation, MODULEINFO},
        Threading::GetCurrentProcess,
    };
    keep_code_loaded().unwrap();
    let first = unsafe { GetModuleHandleW(w!("GraphicsCapture.dll")) }.unwrap();
    for _ in 0..16 {
        std::thread::spawn(|| {
            let apartment = super::capture::Apartment::new().unwrap();
            assert!(super::activation::supported().unwrap());
            drop(apartment);
        })
        .join()
        .unwrap();
        assert_eq!(
            unsafe { GetModuleHandleW(w!("GraphicsCapture.dll")) }.unwrap(),
            first
        );
    }
    let mut info = MODULEINFO::default();
    unsafe {
        GetModuleInformation(
            GetCurrentProcess(),
            first,
            &mut info,
            std::mem::size_of::<MODULEINFO>() as u32,
        )
        .unwrap();
    }
    assert!(info.SizeOfImage > 0);
    println!(
        "wgc_retained_code_image_bytes={} scope=module_mapping_not_gpu_memory_or_working_set",
        info.SizeOfImage
    );
}
