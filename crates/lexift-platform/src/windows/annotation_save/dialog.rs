//! Native dialog validation happens before closure, so overwrite always targets the displayed path.
use super::{native_error, wide};
use lexift_core::{
    Result,
    ports::annotation_save::{ImageFormat, ImageSaveOptions},
};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};
use windows::{
    Win32::{
        Foundation::S_FALSE,
        System::Com::CoTaskMemFree,
        UI::Shell::{
            CDCS_ENABLED, CDCS_VISIBLE, Common::COMDLG_FILTERSPEC, FDE_OVERWRITE_RESPONSE,
            FDE_SHAREVIOLATION_RESPONSE, FDEOR_DEFAULT, FDESVR_DEFAULT, IFileDialog,
            IFileDialogCustomize, IFileDialogEvents, IFileDialogEvents_Vtbl, IFileSaveDialog,
            SIGDN_FILESYSPATH,
        },
    },
    core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, Ref},
};
const ERROR_CONTROL: u32 = 100;

pub(super) fn selected_format(
    dialog: &IFileDialog,
    options: &ImageSaveOptions,
) -> windows::core::Result<ImageFormat> {
    let index = unsafe { dialog.GetFileTypeIndex()? };
    options
        .formats
        .get(index.wrapping_sub(1) as usize)
        .copied()
        .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))
}
pub(super) fn extension_matches(path: &Path, format: ImageFormat) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| format.accepts_extension(e))
}

// Check the edit text as well as GetResult: the shell may append the selected
// extension to a mismatched typed suffix before delivering the final path.
fn typed_extension_matches(path: &Path, format: ImageFormat) -> bool {
    path.extension().is_none() || extension_matches(path, format)
}

/// Only replace supported image suffixes; preserve the base name and JPEG aliases.
fn name_for_format(name: &str, format: ImageFormat) -> Option<PathBuf> {
    let path = Path::new(name);
    let extension = path.extension()?.to_str()?;
    if !format.accepts_extension(extension)
        && [ImageFormat::Png, ImageFormat::Jpeg]
            .iter()
            .any(|f| f.accepts_extension(extension))
    {
        Some(path.with_extension(format.extension()))
    } else {
        None
    }
}

pub(super) struct Subscription {
    dialog: IFileSaveDialog,
    cookie: u32,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            let _ = self.dialog.Unadvise(self.cookie);
        }
    }
}

pub(super) fn configure(
    dialog: &IFileSaveDialog,
    options: &ImageSaveOptions,
) -> Result<Subscription> {
    let filters: Vec<_> = options
        .formats
        .iter()
        .map(|format| match format {
            ImageFormat::Png => COMDLG_FILTERSPEC {
                pszName: windows::core::w!("PNG (*.png)"),
                pszSpec: windows::core::w!("*.png"),
            },
            ImageFormat::Jpeg => COMDLG_FILTERSPEC {
                pszName: windows::core::w!("JPEG (*.jpg;*.jpeg)"),
                pszSpec: windows::core::w!("*.jpg;*.jpeg"),
            },
        })
        .collect();
    unsafe {
        dialog.SetFileTypes(&filters).map_err(native_error)?;
        dialog
            .SetFileTypeIndex(
                options
                    .formats
                    .iter()
                    .position(|f| *f == options.default_format)
                    .unwrap() as u32
                    + 1,
            )
            .map_err(native_error)?;
        let extension = wide(options.default_format.extension().as_ref());
        dialog
            .SetDefaultExtension(PCWSTR(extension.as_ptr()))
            .map_err(native_error)?;
        let custom: IFileDialogCustomize = dialog.cast().map_err(native_error)?;
        custom
            .AddText(ERROR_CONTROL, windows::core::w!(""))
            .map_err(native_error)?;
        custom
            .SetControlState(ERROR_CONTROL, CDCS_ENABLED)
            .map_err(native_error)?;
        let handler = DialogEvents::create(options.clone());
        let cookie = dialog.Advise(&handler).map_err(native_error)?;
        Ok(Subscription {
            dialog: dialog.clone(),
            cookie,
        })
    }
}
// A small STA-owned COM sink avoids introducing a windows-core dependency solely
// for the implementation macro. Advise holds a reference until Unadvise.
#[repr(C)]
struct DialogEvents {
    vtable: &'static IFileDialogEvents_Vtbl,
    references: AtomicU32,
    options: ImageSaveOptions,
    updating_name: Cell<bool>,
}
impl DialogEvents {
    fn create(options: ImageSaveOptions) -> IFileDialogEvents {
        let object = Box::new(Self {
            vtable: &EVENTS_VTABLE,
            references: AtomicU32::new(1),
            options,
            updating_name: Cell::new(false),
        });
        unsafe { IFileDialogEvents::from_raw(Box::into_raw(object).cast()) }
    }

    fn file_ok(&self, pfd: Ref<IFileDialog>) -> windows::core::Result<()> {
        let dialog = pfd.ok()?;
        unsafe {
            let raw_name = dialog.GetFileName()?;
            let typed_name = raw_name.to_string();
            CoTaskMemFree(Some(raw_name.0.cast()));
            let typed_name = typed_name?;
            let item = dialog.GetResult()?;
            let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = raw.to_string();
            CoTaskMemFree(Some(raw.0.cast()));
            let path = path?;
            let format = selected_format(dialog, &self.options)?;
            if !typed_extension_matches(Path::new(&typed_name), format)
                || !extension_matches(Path::new(&path), format)
            {
                let custom: IFileDialogCustomize = dialog.cast()?;
                let message = wide(self.options.extension_mismatch_message.as_ref());
                custom.SetControlLabel(ERROR_CONTROL, PCWSTR(message.as_ptr()))?;
                custom.SetControlState(ERROR_CONTROL, CDCS_ENABLED | CDCS_VISIBLE)?;
                return Err(windows::core::Error::from_hresult(S_FALSE));
            }
        }
        Ok(())
    }
    fn type_change(&self, pfd: Ref<IFileDialog>) -> windows::core::Result<()> {
        let dialog = pfd.ok()?;
        // Shell updates can reenter this sink. Keep the chosen type and name in
        // one bounded change, without stealing focus or posting a delayed rename.
        if self.updating_name.replace(true) {
            return Ok(());
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&self.updating_name);
        let format = selected_format(dialog, &self.options)?;
        unsafe {
            let extension = wide(format.extension().as_ref());
            dialog.SetDefaultExtension(PCWSTR(extension.as_ptr()))?;
            if let Ok(raw) = dialog.GetFileName() {
                let name = raw.to_string();
                CoTaskMemFree(Some(raw.0.cast()));
                if let Some(name) = name_for_format(&name?, format) {
                    let name = wide(name.as_os_str());
                    dialog.SetFileName(PCWSTR(name.as_ptr()))?;
                }
            }
            let custom: IFileDialogCustomize = dialog.cast()?;
            custom.SetControlState(ERROR_CONTROL, CDCS_ENABLED)?;
        }
        Ok(())
    }
}

unsafe extern "system" fn query_interface(
    this: *mut std::ffi::c_void,
    iid: *const GUID,
    object: *mut *mut std::ffi::c_void,
) -> HRESULT {
    if iid.is_null() || object.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    unsafe {
        *object = std::ptr::null_mut();
        if *iid == IUnknown::IID || *iid == IFileDialogEvents::IID {
            *object = this;
            add_ref(this);
            windows::Win32::Foundation::S_OK
        } else {
            windows::Win32::Foundation::E_NOINTERFACE
        }
    }
}
unsafe extern "system" fn add_ref(this: *mut std::ffi::c_void) -> u32 {
    unsafe {
        (&*this.cast::<DialogEvents>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}
unsafe extern "system" fn release(this: *mut std::ffi::c_void) -> u32 {
    let remaining = unsafe {
        (&*this.cast::<DialogEvents>())
            .references
            .fetch_sub(1, Ordering::Release)
            - 1
    };
    if remaining == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(this.cast::<DialogEvents>()));
        }
    }
    remaining
}
unsafe extern "system" fn file_ok(
    this: *mut std::ffi::c_void,
    dialog: *mut std::ffi::c_void,
) -> HRESULT {
    unsafe {
        (&*this.cast::<DialogEvents>())
            .file_ok(std::mem::transmute::<*mut std::ffi::c_void, Ref<IFileDialog>>(dialog))
            .into()
    }
}
unsafe extern "system" fn type_change(
    this: *mut std::ffi::c_void,
    dialog: *mut std::ffi::c_void,
) -> HRESULT {
    unsafe {
        (&*this.cast::<DialogEvents>())
            .type_change(std::mem::transmute::<*mut std::ffi::c_void, Ref<IFileDialog>>(dialog))
            .into()
    }
}
unsafe extern "system" fn folder_changing(
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
) -> HRESULT {
    windows::Win32::Foundation::S_OK
}
unsafe extern "system" fn notify(_: *mut std::ffi::c_void, _: *mut std::ffi::c_void) -> HRESULT {
    windows::Win32::Foundation::S_OK
}
unsafe extern "system" fn share_violation(
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    response: *mut FDE_SHAREVIOLATION_RESPONSE,
) -> HRESULT {
    if response.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    unsafe {
        *response = FDESVR_DEFAULT;
    }
    windows::Win32::Foundation::S_OK
}
unsafe extern "system" fn overwrite(
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    response: *mut FDE_OVERWRITE_RESPONSE,
) -> HRESULT {
    if response.is_null() {
        return windows::Win32::Foundation::E_POINTER;
    }
    unsafe {
        *response = FDEOR_DEFAULT;
    }
    windows::Win32::Foundation::S_OK
}
static EVENTS_VTABLE: IFileDialogEvents_Vtbl = IFileDialogEvents_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: query_interface,
        AddRef: add_ref,
        Release: release,
    },
    OnFileOk: file_ok,
    OnFolderChanging: folder_changing,
    OnFolderChange: notify,
    OnSelectionChange: notify,
    OnShareViolation: share_violation,
    OnTypeChange: type_change,
    OnOverwrite: overwrite,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extensions_are_case_insensitive_without_accepting_other_encodings() {
        for name in ["image.jpg", "中文.JPEG", "image.JPG"] {
            assert!(extension_matches(Path::new(name), ImageFormat::Jpeg));
        }
        for name in ["image", "image.png", "image.jpg.tmp", "image."] {
            assert!(!extension_matches(Path::new(name), ImageFormat::Jpeg));
        }
        assert!(extension_matches(Path::new("image.PNG"), ImageFormat::Png));
        assert!(typed_extension_matches(
            Path::new("image"),
            ImageFormat::Png
        ));
        assert!(!typed_extension_matches(
            Path::new("image.jpg"),
            ImageFormat::Png
        ));
    }
    #[test]
    fn type_change_preserves_base_name_and_replaces_only_known_suffixes() {
        for (name, format, expected) in [
            (
                "Lexift-20261023-112614.png",
                ImageFormat::Jpeg,
                "Lexift-20261023-112614.jpg",
            ),
            ("中文.v1.JPEG", ImageFormat::Png, "中文.v1.png"),
            (
                "C:\\images\\capture.PNG",
                ImageFormat::Jpeg,
                "C:\\images\\capture.jpg",
            ),
        ] {
            assert_eq!(name_for_format(name, format), Some(PathBuf::from(expected)));
        }
        for (name, format) in [
            ("image", ImageFormat::Jpeg),
            ("", ImageFormat::Png),
            ("image.jpeg", ImageFormat::Jpeg),
            ("image.JPG", ImageFormat::Jpeg),
            ("image.txt", ImageFormat::Png),
        ] {
            assert_eq!(name_for_format(name, format), None);
        }
    }

    #[test]
    fn com_sink_keeps_identity_and_native_type_change_uses_request_policy() {
        std::thread::spawn(|| {
            let _apartment = super::super::Apartment::new().unwrap();
            let options = ImageSaveOptions::png_and_jpeg("Mismatch".into());
            let sink = DialogEvents::create(options.clone());
            let identity: IUnknown = sink.cast().unwrap();
            let other: IFileDialogEvents = identity.cast().unwrap();
            assert_eq!(sink.as_raw(), other.as_raw());
            drop(identity);
            drop(other);
            unsafe {
                assert_eq!(
                    (Interface::vtable(&sink).OnFileOk)(sink.as_raw(), std::ptr::null_mut()),
                    windows::Win32::Foundation::E_POINTER
                );
                let dialog: IFileSaveDialog = windows::Win32::System::Com::CoCreateInstance(
                    &windows::Win32::UI::Shell::FileSaveDialog,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                )
                .unwrap();
                let subscription = configure(&dialog, &options).unwrap();
                assert_eq!(
                    selected_format(&dialog, &options).unwrap(),
                    ImageFormat::Png
                );
                dialog
                    .SetFileName(windows::core::w!("capture.png"))
                    .unwrap();
                dialog.SetFileTypeIndex(2).unwrap();
                sink.OnTypeChange(&dialog).unwrap();
                let raw = dialog.GetFileName().unwrap();
                let name = raw.to_string().unwrap();
                CoTaskMemFree(Some(raw.0.cast()));
                assert_eq!(name, "capture.jpg");
                dialog.SetFileTypeIndex(1).unwrap();
                sink.OnTypeChange(&dialog).unwrap();
                let raw = dialog.GetFileName().unwrap();
                let name = raw.to_string().unwrap();
                CoTaskMemFree(Some(raw.0.cast()));
                assert_eq!(name, "capture.png");
                dialog.SetFileTypeIndex(2).unwrap();
                sink.OnTypeChange(&dialog).unwrap();
                assert_eq!(
                    selected_format(&dialog, &options).unwrap(),
                    ImageFormat::Jpeg
                );
                drop(subscription);
            }
        })
        .join()
        .unwrap();
    }
}
