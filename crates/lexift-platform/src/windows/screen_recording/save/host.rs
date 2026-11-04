//! Bound Shell's process-global dialog resources to a single native request.
//! Only small metadata and the selected path cross private inherited pipes.
use super::*;
use lexift_core::ports::magnifier::WindowToken;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::AsRawHandle,
        process::CommandExt,
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        System::Threading::*,
        UI::{
            Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

const ROLE: &str = "--internal-recording-save-dialog";
const REQUEST_LIMIT: u64 = 64 * 1024;
const RESPONSE_LIMIT: u64 = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    parent: u32,
    owner: Option<isize>,
    owner_epoch: Option<usize>,
    name: String,
    title: String,
    mismatch: String,
}
#[derive(Serialize, Deserialize)]
enum Reply {
    Selected(Vec<u16>),
    Cancelled,
    Failed,
}

fn unavailable() -> Error {
    Error::new("Video save dialog unavailable")
}

const OWNER_PROPERTY: PCWSTR = w!("Lexift.RecordingDialog.OwnerEpoch");
struct OwnerGuard {
    window: Option<HWND>,
    epoch: usize,
    enabled: bool,
}
impl OwnerGuard {
    fn new(owner: Option<WindowToken>) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let mut guard = Self {
            window: None,
            epoch: NEXT.fetch_add(1, Ordering::Relaxed).max(1),
            enabled: false,
        };
        if let Some(owner) = owner {
            let hwnd = HWND(owner.0 as *mut _);
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if pid != std::process::id() {
                return Err(unavailable());
            }
            guard.enabled = unsafe { IsWindowEnabled(hwnd).as_bool() };
            unsafe { SetPropW(hwnd, OWNER_PROPERTY, Some(HANDLE(guard.epoch as *mut _))) }
                .map_err(|_| unavailable())?;
            guard.window = Some(hwnd);
        }
        Ok(guard)
    }
    fn current(&self) -> bool {
        self.window
            .is_none_or(|w| unsafe { GetPropW(w, OWNER_PROPERTY).0 as usize == self.epoch })
    }
}
impl Drop for OwnerGuard {
    fn drop(&mut self) {
        if let Some(window) = self.window
            && self.current()
        {
            unsafe {
                let _ = RemovePropW(window, OWNER_PROPERTY);
                // A terminated modal host cannot run Show's owner restoration.
                // Enable only this request's surviving owner, without activation.
                if self.enabled && !IsWindowEnabled(window).as_bool() {
                    let _ = EnableWindow(window, true);
                }
            }
        }
    }
}

fn decode_reply(bytes: &[u8]) -> Result<Option<PathBuf>> {
    if bytes.len() as u64 > RESPONSE_LIMIT {
        return Err(unavailable());
    }
    match serde_json::from_slice(bytes).map_err(|_| unavailable())? {
        Reply::Selected(wide) if !wide.is_empty() && wide.len() <= 32767 && !wide.contains(&0) => {
            let path = PathBuf::from(std::ffi::OsString::from_wide(&wide));
            if !path.is_absolute()
                || !path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
            {
                return Err(unavailable());
            }
            Ok(Some(path))
        }
        Reply::Cancelled => Ok(None),
        _ => Err(unavailable()),
    }
}

/// Cancellation targets a retained process handle, never a recycled PID.
struct CancellationWindows {
    pid: u32,
    visible: bool,
}
unsafe extern "system" fn cancel_window(hwnd: HWND, pid: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(pid.0 as *mut CancellationWindows);
        let mut owner = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        if owner == state.pid && IsWindowVisible(hwnd).as_bool() {
            state.visible = true;
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    BOOL(1)
}

pub(super) fn choose(request: SaveRequest, cancel: &AtomicBool) -> Result<Option<PathBuf>> {
    let identity = request.identity;
    let started = Instant::now();
    let owner_guard = OwnerGuard::new(request.owner)?;
    let payload = serde_json::to_vec(&Request {
        version: 1,
        parent: std::process::id(),
        owner: request.owner.map(|w| w.0),
        owner_epoch: request.owner.map(|_| owner_guard.epoch),
        name: request.default_name,
        title: request.title,
        mismatch: request.extension_mismatch_message,
    })
    .map_err(|_| unavailable())?;
    if payload.len() as u64 > REQUEST_LIMIT {
        return Err(unavailable());
    }
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let mut command = Command::new(std::env::current_exe().map_err(|_| unavailable())?);
    command
        .arg(ROLE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0);
    let mut child = command.spawn().map_err(|_| unavailable())?;
    let process = child.as_raw_handle() as isize;
    let pid = child.id();
    tracing::debug!(
        session = identity.session,
        operation = identity.operation,
        request = identity.request,
        helper_pid = pid,
        "Recording dialog helper started"
    );
    let done = AtomicBool::new(false);
    let io_result = std::thread::scope(|scope| {
        scope.spawn(|| {
            let handle = HANDLE(process as *mut _);
            let mut cancelled = None;
            while !done.load(Ordering::Acquire)
                && unsafe { WaitForSingleObject(handle, 20) } == WAIT_TIMEOUT
            {
                if cancel.load(Ordering::Acquire) {
                    let since = cancelled.get_or_insert_with(Instant::now);
                    let mut windows = CancellationWindows {
                        pid,
                        visible: false,
                    };
                    unsafe {
                        let _ = EnumWindows(
                            Some(cancel_window),
                            LPARAM(&mut windows as *mut _ as isize),
                        );
                    }
                    if !windows.visible || since.elapsed() >= Duration::from_secs(2) {
                        // A hung Shell extension cannot retain the main process.
                        // Retire an initializing host before it can show late UI.
                        // This handle still denotes the helper we created.
                        unsafe {
                            let _ = TerminateProcess(handle, ERROR_CANCELLED.0);
                        }
                        break;
                    }
                }
            }
        });
        let result = (|| -> io::Result<_> {
            let mut input = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("Missing dialog pipe"))?;
            input.write_all(&payload)?;
            drop(input);
            let mut output_pipe = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("Missing dialog pipe"))?;
            let mut length = [0; 4];
            output_pipe.read_exact(&mut length)?;
            let length = u32::from_le_bytes(length) as u64;
            if length > RESPONSE_LIMIT {
                return Err(io::Error::other("Oversized dialog response"));
            }
            let mut output = vec![0; length as usize];
            output_pipe.read_exact(&mut output)?;
            // Selection is complete, but success also requires the Shell host
            // to retire. This is a failure deadline, not a presentation delay.
            if unsafe { WaitForSingleObject(HANDLE(process as *mut _), 2000) } != WAIT_OBJECT_0 {
                return Err(io::Error::other("Dialog helper retirement failed"));
            }
            let status = child.wait()?;
            Ok((status, output))
        })();
        // Even transport failure must retire the exact helper before returning.
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        done.store(true, Ordering::Release);
        result
    });
    tracing::debug!(
        session = identity.session,
        operation = identity.operation,
        request = identity.request,
        helper_pid = pid,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
        cancelled = cancel.load(Ordering::Acquire),
        transport_ok = io_result.is_ok(),
        "Recording dialog helper retired"
    );
    if cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    if !owner_guard.current() {
        return Err(unavailable());
    }
    let (status, output) = io_result.map_err(|_| unavailable())?;
    if !status.success() {
        return Err(unavailable());
    }
    decode_reply(&output)
}

pub(crate) fn run_from_args() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(ROLE)) {
        return None;
    }
    if args.next().is_some() {
        return Some(2);
    }
    let result = (|| -> Result<_> {
        unsafe {
            for handle in [
                HANDLE(io::stdin().as_raw_handle()),
                HANDLE(io::stdout().as_raw_handle()),
            ] {
                if GetFileType(handle) != FILE_TYPE_PIPE {
                    return Err(unavailable());
                }
            }
        }
        let mut bytes = Vec::new();
        io::stdin()
            .take(REQUEST_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() as u64 > REQUEST_LIMIT {
            return Err(unavailable());
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if request.version != 1 || request.parent == 0 {
            return Err(unavailable());
        }
        if let Some(owner) = request.owner {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(HWND(owner as *mut _), Some(&mut pid));
            }
            if pid != request.parent
                || request.owner_epoch.is_none_or(|epoch| {
                    epoch == 0
                        || unsafe {
                            GetPropW(HWND(owner as *mut _), OWNER_PROPERTY).0 as usize != epoch
                        }
                })
            {
                return Err(unavailable());
            }
        }
        super::choose_path(
            request.owner.map(WindowToken),
            request.name,
            request.title,
            request.mismatch,
        )
    })();
    let reply = match result {
        Ok(Some(path)) => {
            let wide: Vec<_> = path.as_os_str().encode_wide().take(32768).collect();
            if wide.len() > 32767 {
                Reply::Failed
            } else {
                Reply::Selected(wide)
            }
        }
        Ok(None) => Reply::Cancelled,
        Err(_) => Reply::Failed,
    };
    let Ok(bytes) = serde_json::to_vec(&reply) else {
        return Some(1);
    };
    let okay = io::stdout()
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| io::stdout().write_all(&bytes))
        .and_then(|_| io::stdout().flush())
        .is_ok();
    Some(if okay { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_protocol_rejects_wrong_paths_and_accepts_cancel() {
        assert!(decode_reply(br#""Cancelled""#).unwrap().is_none());
        for wide in [vec![], vec![0], vec![65], vec![65; 32768]] {
            assert!(decode_reply(&serde_json::to_vec(&Reply::Selected(wide)).unwrap()).is_err());
        }
        assert!(decode_reply(&vec![b' '; RESPONSE_LIMIT as usize + 1]).is_err());
        assert!(decode_reply(br#"{"Unknown":true}"#).is_err());
        let path = PathBuf::from(r"C:\owned-fixture\视频.mp4");
        assert_eq!(
            decode_reply(
                &serde_json::to_vec(&Reply::Selected(path.as_os_str().encode_wide().collect()))
                    .unwrap()
            )
            .unwrap(),
            Some(path)
        );
    }

    #[test]
    fn stale_owner_guard_cannot_unblock_a_new_modal_request() {
        unsafe {
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("owned hidden modal guard test"),
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let token = Some(WindowToken(window.0 as isize));
            let old = OwnerGuard::new(token).unwrap();
            let next = OwnerGuard::new(token).unwrap();
            let _ = EnableWindow(window, false);
            drop(old);
            assert!(!IsWindowEnabled(window).as_bool());
            assert!(next.current());
            drop(next);
            assert!(IsWindowEnabled(window).as_bool());
            assert!(GetPropW(window, OWNER_PROPERTY).is_invalid());
            let _ = EnableWindow(window, false);
            drop(OwnerGuard::new(token).unwrap());
            assert!(!IsWindowEnabled(window).as_bool());
            DestroyWindow(window).unwrap();
        }
    }
}
