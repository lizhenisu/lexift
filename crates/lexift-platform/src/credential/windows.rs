use std::{ffi::c_void, ptr, slice};

use lexift_core::ports::credential::{
    CredentialError, CredentialErrorKind, CredentialResult, CredentialStore,
};
use windows::{
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_NOT_FOUND},
        Security::Credentials::{
            CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
            CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
        },
    },
    core::{HRESULT, PCWSTR, PWSTR},
};

const TARGET_PREFIX: &str = "Lexift/";

#[derive(Debug, Default)]
pub(crate) struct WindowsCredentialStore;

impl WindowsCredentialStore {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl CredentialStore for WindowsCredentialStore {
    fn get(&self, id: &str) -> CredentialResult<Option<String>> {
        let target = credential_target(id)?;
        let mut native = ptr::null_mut();
        let read = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut native,
            )
        };
        if let Err(error) = read {
            if is_windows_error(&error, ERROR_NOT_FOUND.0) {
                return Ok(None);
            }
            return Err(map_windows_error(error, "read"));
        }
        let native = NativeCredential::new(native)?;
        let credential = native.as_ref();
        let bytes = if credential.CredentialBlobSize == 0 {
            &[]
        } else if credential.CredentialBlob.is_null() {
            return Err(CredentialError::new(
                CredentialErrorKind::InvalidFormat,
                "Stored credential has an invalid value",
            ));
        } else {
            unsafe {
                slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            }
        };
        String::from_utf8(bytes.to_vec()).map(Some).map_err(|_| {
            CredentialError::new(
                CredentialErrorKind::InvalidFormat,
                "Stored credential is not valid UTF-8",
            )
        })
    }

    fn set(&self, id: &str, secret: &str) -> CredentialResult<()> {
        if secret.is_empty() {
            return Err(CredentialError::new(
                CredentialErrorKind::InvalidFormat,
                "Credential value cannot be empty",
            ));
        }
        if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(CredentialError::new(
                CredentialErrorKind::InvalidFormat,
                "Credential value is too large",
            ));
        }
        let mut target = credential_target(id)?;
        let mut username = wide_string("Lexift");
        let mut secret_bytes = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: secret_bytes.len() as u32,
            CredentialBlob: secret_bytes.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..Default::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) }
            .map_err(|error| map_windows_error(error, "write"));
        secret_bytes.fill(0);
        result
    }

    fn delete(&self, id: &str) -> CredentialResult<()> {
        let target = credential_target(id)?;
        unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) }.map_err(|error| {
            if is_windows_error(&error, ERROR_NOT_FOUND.0) {
                CredentialError::new(CredentialErrorKind::Missing, "Credential does not exist")
            } else {
                map_windows_error(error, "delete")
            }
        })
    }
}

struct NativeCredential(*mut CREDENTIALW);

impl NativeCredential {
    fn new(value: *mut CREDENTIALW) -> CredentialResult<Self> {
        if value.is_null() {
            Err(CredentialError::new(
                CredentialErrorKind::PlatformFailure,
                "Credential Manager returned an empty result",
            ))
        } else {
            Ok(Self(value))
        }
    }

    fn as_ref(&self) -> &CREDENTIALW {
        unsafe { &*self.0 }
    }
}

impl Drop for NativeCredential {
    fn drop(&mut self) {
        unsafe { CredFree(self.0.cast::<c_void>()) };
    }
}

fn credential_target(id: &str) -> CredentialResult<Vec<u16>> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
    {
        return Err(CredentialError::new(
            CredentialErrorKind::InvalidFormat,
            "Credential identifier is invalid",
        ));
    }
    Ok(wide_string(&format!("{TARGET_PREFIX}{id}")))
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn is_windows_error(error: &windows::core::Error, code: u32) -> bool {
    error.code() == HRESULT::from_win32(code)
}

fn map_windows_error(error: windows::core::Error, operation: &str) -> CredentialError {
    let kind = if is_windows_error(&error, ERROR_ACCESS_DENIED.0) {
        CredentialErrorKind::PermissionDenied
    } else {
        CredentialErrorKind::PlatformFailure
    };
    CredentialError::new(
        kind,
        format!("Credential Manager could not {operation} the credential"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_use_the_stable_lexift_namespace() {
        let target = credential_target("deepl-primary").unwrap();
        let value = String::from_utf16(&target[..target.len() - 1]).unwrap();
        assert_eq!(value, "Lexift/deepl-primary");
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        for id in ["", "../secret", "deepl/account", "deepl primary"] {
            assert_eq!(
                credential_target(id).unwrap_err().kind(),
                CredentialErrorKind::InvalidFormat
            );
        }
    }

    #[test]
    #[ignore = "writes a temporary entry to Windows Credential Manager"]
    fn credential_manager_round_trip() {
        let store = WindowsCredentialStore::new();
        let id = format!("integration-{}", std::process::id());
        let _ = store.delete(&id);
        store.set(&id, "temporary-test-secret").unwrap();
        assert_eq!(
            store.get(&id).unwrap().as_deref(),
            Some("temporary-test-secret")
        );
        store.delete(&id).unwrap();
        assert_eq!(store.get(&id).unwrap(), None);
    }
}
