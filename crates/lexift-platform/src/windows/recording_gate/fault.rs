//! Owned quota stream: native MF writes fail without filling a real disk.
#![allow(non_snake_case)]
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{Foundation::*, System::Com::*},
    core::*,
};

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    Create,
    Write,
    Finalize,
    Rename,
    DeviceLost,
    CaptureClosed,
    ResumeStale,
    CancelFinalize,
}

#[implement(IStream)]
pub struct Quota {
    file: Mutex<File>,
    pub armed: Arc<AtomicBool>,
    failed: Arc<AtomicU64>,
}
impl Quota {
    pub fn new(file: File, armed: Arc<AtomicBool>, failed: Arc<AtomicU64>) -> Self {
        Self {
            file: Mutex::new(file),
            armed,
            failed,
        }
    }
}
fn io(e: std::io::Error) -> Error {
    Error::new(E_FAIL, e.to_string())
}
impl ISequentialStream_Impl for Quota_Impl {
    fn Read(&self, p: *mut std::ffi::c_void, n: u32, read: *mut u32) -> HRESULT {
        if n == 0 {
            if !read.is_null() {
                unsafe {
                    *read = 0;
                }
            }
            return S_OK;
        }
        if p.is_null() {
            return E_POINTER;
        }
        let result = unsafe {
            self.file
                .lock()
                .unwrap()
                .read(std::slice::from_raw_parts_mut(p.cast(), n as usize))
        };
        match result {
            Ok(count) => {
                if !read.is_null() {
                    unsafe {
                        *read = count as u32;
                    }
                }
                if count < n as usize { S_FALSE } else { S_OK }
            }
            Err(_) => E_FAIL,
        }
    }
    fn Write(&self, p: *const std::ffi::c_void, n: u32, written: *mut u32) -> HRESULT {
        if n == 0 {
            if !written.is_null() {
                unsafe {
                    *written = 0;
                }
            }
            return S_OK;
        }
        if p.is_null() {
            return E_POINTER;
        }
        if self.armed.load(Ordering::Acquire) {
            self.failed.fetch_add(1, Ordering::Relaxed);
            if !written.is_null() {
                unsafe {
                    *written = 0;
                }
            }
            return STG_E_MEDIUMFULL;
        }
        let result = unsafe {
            self.file
                .lock()
                .unwrap()
                .write_all(std::slice::from_raw_parts(p.cast(), n as usize))
        };
        if result.is_ok() {
            if !written.is_null() {
                unsafe {
                    *written = n;
                }
            }
            S_OK
        } else {
            STG_E_WRITEFAULT
        }
    }
}
impl IStream_Impl for Quota_Impl {
    fn Seek(&self, offset: i64, origin: STREAM_SEEK, position: *mut u64) -> Result<()> {
        let from = match origin {
            STREAM_SEEK_SET => SeekFrom::Start(
                u64::try_from(offset).map_err(|_| Error::from_hresult(E_INVALIDARG))?,
            ),
            STREAM_SEEK_CUR => SeekFrom::Current(offset),
            STREAM_SEEK_END => SeekFrom::End(offset),
            _ => return Err(Error::from_hresult(E_INVALIDARG)),
        };
        let value = self.file.lock().unwrap().seek(from).map_err(io)?;
        if !position.is_null() {
            unsafe {
                *position = value;
            }
        }
        Ok(())
    }
    fn SetSize(&self, size: u64) -> Result<()> {
        self.file.lock().unwrap().set_len(size).map_err(io)
    }
    fn CopyTo(&self, _: Ref<IStream>, _: u64, _: *mut u64, _: *mut u64) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn Commit(&self, _: &STGC) -> Result<()> {
        self.file.lock().unwrap().flush().map_err(io)
    }
    fn Revert(&self) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn Stat(&self, out: *mut STATSTG, _: &STATFLAG) -> Result<()> {
        if out.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        unsafe {
            *out = STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: self.file.lock().unwrap().metadata().map_err(io)?.len(),
                grfMode: STGM_READWRITE,
                ..Default::default()
            };
        }
        Ok(())
    }
    fn Clone(&self) -> Result<IStream> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
}
