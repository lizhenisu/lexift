//! Versioned, bounded DPAPI ciphertext storage. Failed replacement leaves the previous file intact.
use lexift_core::{
    Error, Result,
    domain::clipboard_history::{HistoryEntry, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES},
    ports::clipboard_history::ClipboardHistoryStore,
};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    },
    core::PCWSTR,
};
pub(crate) struct Store {
    pub path: PathBuf,
}
fn error() -> Error {
    Error::new("Encrypted clipboard history storage is unavailable")
}
fn crypt(bytes: &[u8], protect: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| error())?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if protect {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    }
    .map_err(|_| error())?;
    struct Allocation(*mut u8);
    impl Drop for Allocation {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.cast())));
            }
        }
    }
    let allocation = Allocation(output.pbData);
    if output.cbData as usize > MAX_TOTAL_BYTES + 65536 || allocation.0.is_null() {
        return Err(error());
    }
    Ok(unsafe { std::slice::from_raw_parts(allocation.0, output.cbData as usize) }.to_vec())
}
fn encode(entries: &[HistoryEntry]) -> Result<Vec<u8>> {
    if entries.len() > 200 || entries.iter().map(|e| e.text.len()).sum::<usize>() > MAX_TOTAL_BYTES
    {
        return Err(error());
    }
    let mut bytes = b"LXCH\x01".to_vec();
    bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        if e.text.is_empty() || e.text.len() > MAX_ENTRY_BYTES {
            return Err(error());
        }
        bytes.extend_from_slice(&e.id.to_le_bytes());
        bytes.extend_from_slice(&(e.text.len() as u32).to_le_bytes());
        bytes.extend_from_slice(e.text.as_bytes());
    }
    Ok(bytes)
}
fn decode(bytes: &[u8]) -> Result<Vec<HistoryEntry>> {
    let mut input = bytes;
    fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
        if count > input.len() {
            return Err(error());
        }
        let (a, b) = input.split_at(count);
        *input = b;
        Ok(a)
    }
    if take(&mut input, 5)? != b"LXCH\x01" {
        return Err(error());
    }
    let count = u32::from_le_bytes(take(&mut input, 4)?.try_into().unwrap()) as usize;
    if count > 200 {
        return Err(error());
    }
    let mut entries = Vec::with_capacity(count);
    let mut total = 0;
    for _ in 0..count {
        let id = u64::from_le_bytes(take(&mut input, 8)?.try_into().unwrap());
        let len = u32::from_le_bytes(take(&mut input, 4)?.try_into().unwrap()) as usize;
        total += len;
        if len == 0
            || len > MAX_ENTRY_BYTES
            || total > MAX_TOTAL_BYTES
            || entries.iter().any(|e: &HistoryEntry| e.id == id)
        {
            return Err(error());
        }
        let text = std::str::from_utf8(take(&mut input, len)?).map_err(|_| error())?;
        entries.push(HistoryEntry {
            id,
            text: Arc::from(text),
        });
    }
    if !input.is_empty() {
        return Err(error());
    }
    Ok(entries)
}
impl ClipboardHistoryStore for Store {
    fn load(&self) -> Result<Vec<HistoryEntry>> {
        match fs::metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Ok(m) if m.len() <= (MAX_TOTAL_BYTES + 65536) as u64 => {}
            _ => return Err(error()),
        }
        let mut encrypted = Vec::new();
        fs::File::open(&self.path)
            .map_err(|_| error())?
            .take((MAX_TOTAL_BYTES + 65537) as u64)
            .read_to_end(&mut encrypted)
            .map_err(|_| error())?;
        if encrypted.len() > MAX_TOTAL_BYTES + 65536 {
            return Err(error());
        }
        let mut plain = crypt(&encrypted, false)?;
        let result = decode(&plain);
        plain.fill(0);
        result
    }
    fn save(&self, entries: &[HistoryEntry]) -> Result<()> {
        if entries.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(error()),
            };
        }
        let mut plain = encode(entries)?;
        let encrypted = crypt(&plain, true);
        plain.fill(0);
        let encrypted = encrypted?;
        let parent = self.path.parent().ok_or_else(error)?;
        fs::create_dir_all(parent).map_err(|_| error())?;
        let temp = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = fs::File::create(&temp).map_err(|_| error())?;
            file.write_all(&encrypted).map_err(|_| error())?;
            file.sync_all().map_err(|_| error())?;
            drop(file);
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = self.path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
            .map_err(|_| error())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}
use std::os::windows::ffi::OsStrExt;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_roundtrip_and_corruption_preserve_old_file() {
        let path = std::env::temp_dir().join(format!("lexift-history-test-{}", std::process::id()));
        let store = Store { path: path.clone() };
        let entries = vec![HistoryEntry {
            id: 7,
            text: Arc::from("private 中文\ntext"),
        }];
        store.save(&entries).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes.windows("private".len()).any(|b| b == b"private"));
        assert_eq!(store.load().unwrap(), entries);
        fs::write(&path, b"broken").unwrap();
        assert!(store.load().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
        store.save(&[]).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn bounded_decoder_rejects_truncated_and_extra_bytes() {
        let e = vec![HistoryEntry {
            id: 1,
            text: Arc::from("hello"),
        }];
        let bytes = encode(&e).unwrap();
        assert_eq!(decode(&bytes).unwrap(), e);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(decode(&extra).is_err());
    }
}
