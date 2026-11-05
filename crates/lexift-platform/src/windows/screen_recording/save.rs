//! Isolated format selection, bounded MP4/GIF export and atomic publication.
mod host;
pub(crate) use host::run_from_args;
use lexift_core::{
    Error, Result,
    ports::{annotation_save::SaveOutcome, screen_recording::SaveRequest},
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{Foundation::*, Storage::FileSystem::*, System::Com::*, UI::Shell::*},
    core::*,
};
fn error(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain([0]).collect()
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub path: PathBuf,
    pub format: lexift_core::domain::recording_gif::Format,
}
struct GifTemporary(PathBuf);
impl Drop for GifTemporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn gif_temporary(destination: &Path) -> Result<GifTemporary> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let path = destination.with_file_name(format!(
        ".lexift-{}-gif-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(error)?;
    Ok(GifTemporary(path))
}
pub fn save(
    source: &Path,
    request: SaveRequest,
    cancel: Arc<AtomicBool>,
    info: &super::ArtifactInfo,
    listener: super::Listener,
) -> Result<SaveOutcome> {
    if cancel.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    let range = request
        .range
        .unwrap_or(lexift_core::domain::recording_trim::Range::full(
            info.frames,
        ));
    if !range.valid(info.frames) {
        return Err(Error::new("Invalid recording trim range"));
    }
    let gif_plan = request.gif_plan.clone();
    let identity = request.identity;
    let range_version = request.range_version;
    let selected = host::choose(request, &cancel)?;
    if let Some(Selection {
        path,
        format: lexift_core::domain::recording_gif::Format::Gif,
    }) = &selected
    {
        let plan = gif_plan.map_err(|_| Error::new("GIF export unavailable for this range"))?;
        let temp = gif_temporary(path)?;
        let result = super::gif::export(
            source,
            &temp.0,
            info,
            &plan,
            &cancel,
            |phase, completed, total| {
                let stage = match phase {
                    super::gif::Phase::Analyzing => super::ExportStage::AnalyzingColors,
                    super::gif::Phase::Encoding => super::ExportStage::EncodingGif,
                    super::gif::Phase::Finalizing => super::ExportStage::Finalizing,
                };
                listener(super::Notice::ExportProgress {
                    identity,
                    range_version,
                    stage,
                    completed,
                    total,
                });
            },
        );
        if cancel.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        result?;
        listener(super::Notice::ExportProgress {
            identity,
            range_version,
            stage: super::ExportStage::Publishing,
            completed: plan.frames,
            total: plan.frames,
        });
        return publish(&temp.0, path, &cancel);
    }
    match selected.map(|s| s.path) {
        Some(path) if range == lexift_core::domain::recording_trim::Range::full(info.frames) => {
            publish(source, &path, &cancel)
        }
        Some(path) => {
            let file = super::FileArtifact::new()?;
            let progress = |stage, completed, total| {
                listener(super::Notice::ExportProgress {
                    identity,
                    range_version,
                    stage,
                    completed,
                    total,
                })
            };
            match super::trim::export(source, &file.path, info, range, &cancel, progress) {
                Ok(()) if !cancel.load(Ordering::Acquire) => {
                    listener(super::Notice::ExportProgress {
                        identity,
                        range_version,
                        stage: super::ExportStage::Publishing,
                        completed: range.len(),
                        total: range.len(),
                    });
                    publish(&file.path, &path, &cancel)
                }
                _ if cancel.load(Ordering::Acquire) => Ok(SaveOutcome::Cancelled),
                Err(error) => Err(error),
                Ok(()) => Ok(SaveOutcome::Cancelled),
            }
        }
        None => Ok(SaveOutcome::Cancelled),
    }
}

/// The helper owns the dialog's entire STA and Shell lifetime; no video is sent.
fn choose_path(
    owner: Option<lexift_core::ports::magnifier::WindowToken>,
    name: String,
    title: String,
    options: super::super::annotation_save::dialog::RecordingOptions,
) -> Result<Option<Selection>> {
    let _shell = super::super::window_shell_policy::ThreadGuard::new()?;
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(error)?;
    let _apartment = Apartment;
    let dialog: IFileSaveDialog =
        unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }.map_err(error)?;
    let name = Path::new(&name).with_extension("");
    let name = wide(name.as_os_str());
    let title = wide(title.as_ref());
    unsafe {
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(error)?
                    | FOS_FORCEFILESYSTEM
                    | FOS_OVERWRITEPROMPT
                    | FOS_PATHMUSTEXIST
                    | FOS_STRICTFILETYPES,
            )
            .map_err(error)?;
        dialog.SetTitle(PCWSTR(title.as_ptr())).map_err(error)?;
        dialog.SetFileName(PCWSTR(name.as_ptr())).map_err(error)?;
    }
    let _events =
        super::super::annotation_save::dialog::configure_recording(&dialog, options.clone())?;
    let shown = unsafe { dialog.Show(owner.map(|t| HWND(t.0 as *mut _))) };
    if shown
        .as_ref()
        .is_err_and(|e| e.code() == HRESULT::from_win32(ERROR_CANCELLED.0))
    {
        return Ok(None);
    }
    shown.map_err(error)?;
    let item = unsafe { dialog.GetResult() }.map_err(error)?;
    let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(error)?;
    let path = unsafe { raw.to_string() };
    unsafe {
        CoTaskMemFree(Some(raw.0.cast()));
    }
    let path = PathBuf::from(path.map_err(error)?);
    let format = options.selected(&dialog).map_err(error)?;
    if !path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| format.accepts_extension(e))
    {
        return Err(Error::new("Video extension does not match selected format"));
    }
    Ok(Some(Selection { path, format }))
}

pub fn publish(source: &Path, destination: &Path, cancel: &AtomicBool) -> Result<SaveOutcome> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let temp = destination.with_file_name(format!(
        ".lexift-{}-{}.video.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    let input = std::fs::File::open(source).map_err(error)?;
    let output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(error)?;
    let _temp = Temporary(temp.clone());
    {
        let mut input = input;
        let mut output = output;
        let mut buffer = [0u8; 65536];
        loop {
            if cancel.load(Ordering::Acquire) {
                return Ok(SaveOutcome::Cancelled);
            }
            let n = input.read(&mut buffer).map_err(error)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).map_err(error)?;
        }
        output.sync_all().map_err(error)?;
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(SaveOutcome::Cancelled);
    }
    let from = wide(temp.as_os_str());
    let to = wide(destination.as_os_str());
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(error)?;
    Ok(SaveOutcome::Saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publishing_is_atomic_cancellable_and_preserves_the_sealed_source() {
        let directory = std::env::temp_dir().join(format!(
            "lexift-video-publish-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("source.mp4");
        let output = directory.join("output.mp4");
        let data = vec![37; 200_000];
        std::fs::write(&source, &data).unwrap();
        std::fs::write(&output, b"original").unwrap();
        assert_eq!(
            publish(&source, &output, &AtomicBool::new(true)).unwrap(),
            SaveOutcome::Cancelled
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        assert!(
            publish(
                &directory.join("missing.mp4"),
                &output,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        assert_eq!(
            publish(&source, &output, &AtomicBool::new(false)).unwrap(),
            SaveOutcome::Saved
        );
        assert_eq!(std::fs::read(&output).unwrap(), data);
        assert_eq!(std::fs::read(&source).unwrap(), data);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
