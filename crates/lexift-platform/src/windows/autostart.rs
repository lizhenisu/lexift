use std::{mem::size_of_val, path::PathBuf};

use lexift_core::{Error, Result, ports::autostart::AutostartPort};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
            REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
            RegSetValueExW,
        },
    },
    core::{PCWSTR, w},
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("Lexift");

pub(crate) struct WindowsAutostartPort {
    executable: PathBuf,
}

impl WindowsAutostartPort {
    pub(crate) fn new() -> Result<Self> {
        let executable = std::env::current_exe()
            .map_err(|error| Error::new(format!("Could not resolve Lexift executable: {error}")))?;
        Ok(Self { executable })
    }

    fn command(&self) -> String {
        format!("\"{}\" --background", self.executable.display())
    }
}

impl AutostartPort for WindowsAutostartPort {
    fn set_enabled(&self, enabled: bool) -> Result<()> {
        if enabled {
            write_run_value(&self.command())
        } else {
            delete_run_value()
        }
    }

    fn is_enabled(&self) -> Result<bool> {
        Ok(read_run_value()?.as_deref() == Some(self.command().as_str()))
    }
}

fn write_run_value(command: &str) -> Result<()> {
    let key = create_run_key()?;
    let _key = RegistryKey(key);
    let wide = wide_string(command);
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), size_of_val(wide.as_slice()))
    };
    let result = unsafe { RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes)) };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(registry_error("write", result))
    }
}

fn delete_run_value() -> Result<()> {
    let key = match open_run_key(KEY_SET_VALUE) {
        Ok(key) => key,
        Err(error) if error == ERROR_FILE_NOT_FOUND => return Ok(()),
        Err(error) => return Err(registry_error("open", error)),
    };
    let _key = RegistryKey(key);
    let result = unsafe { RegDeleteValueW(key, VALUE_NAME) };
    if result == ERROR_SUCCESS || result == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(registry_error("delete", result))
    }
}

fn read_run_value() -> Result<Option<String>> {
    let key = match open_run_key(KEY_QUERY_VALUE) {
        Ok(key) => key,
        Err(error) if error == ERROR_FILE_NOT_FOUND => return Ok(None),
        Err(error) => return Err(registry_error("open", error)),
    };
    let _key = RegistryKey(key);
    let mut value_type = Default::default();
    let mut byte_len = 0u32;
    let result = unsafe {
        RegQueryValueExW(
            key,
            VALUE_NAME,
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if result != ERROR_SUCCESS {
        return Err(registry_error("read", result));
    }
    if value_type != REG_SZ {
        return Ok(None);
    }
    let mut bytes = vec![0u8; byte_len as usize];
    let result = unsafe {
        RegQueryValueExW(
            key,
            VALUE_NAME,
            None,
            Some(&mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&mut byte_len),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(registry_error("read", result));
    }
    let words = bytes[..byte_len as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|word| *word != 0)
        .collect::<Vec<_>>();
    Ok(Some(String::from_utf16_lossy(&words)))
}

fn create_run_key() -> Result<HKEY> {
    let mut key = HKEY::default();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if result == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(registry_error("create", result))
    }
}

fn open_run_key(
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> std::result::Result<HKEY, windows::Win32::Foundation::WIN32_ERROR> {
    let mut key = HKEY::default();
    let result = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    if result == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(result)
    }
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn registry_error(action: &str, error: windows::Win32::Foundation::WIN32_ERROR) -> Error {
    Error::new(format!(
        "Could not {action} the Windows launch-at-login entry (error {})",
        error.0
    ))
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_the_executable_and_uses_background_mode() {
        let port = WindowsAutostartPort {
            executable: PathBuf::from(r"C:\Program Files\Lexift\lexift-app.exe"),
        };
        assert_eq!(
            port.command(),
            r#""C:\Program Files\Lexift\lexift-app.exe" --background"#
        );
    }

    #[test]
    #[ignore = "temporarily updates the current user's Lexift startup registry value"]
    fn startup_registry_round_trip_restores_the_previous_value() {
        struct Restore(Option<String>);

        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = match &self.0 {
                    Some(value) => write_run_value(value),
                    None => delete_run_value(),
                };
            }
        }

        let _restore = Restore(read_run_value().unwrap());
        let port = WindowsAutostartPort {
            executable: PathBuf::from(r"C:\Program Files\Lexift Test\lexift.exe"),
        };
        port.set_enabled(true).unwrap();
        assert!(port.is_enabled().unwrap());
        port.set_enabled(false).unwrap();
        assert!(!port.is_enabled().unwrap());
    }
}
