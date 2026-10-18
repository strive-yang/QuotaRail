//! Manage only QuotaRail's current-user Run value. No automatic registration.
use super::windows::wide;
use std::{
    io,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

pub struct StartupEntry {
    key: String,
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn checked(code: WIN32_ERROR) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        out.push_str(&"\\".repeat(if ch == '"' { slashes * 2 + 1 } else { slashes }));
        out.push(ch);
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}
pub fn command(executable: &Path, data_dir: &Path) -> io::Result<String> {
    if !executable.is_absolute() || !data_dir.is_absolute() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let exe = executable.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    let data = data_dir.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    if exe.contains(['\0', '\r', '\n', '"']) || data.contains(['\0', '\r', '\n', '"']) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(format!("{} --data-dir {}", quote(exe), quote(data)))
}
impl Default for StartupEntry {
    fn default() -> Self {
        #[cfg(feature = "test-support")]
        if let Ok(key) = std::env::var("QUOTARAIL_TEST_STARTUP_KEY")
            && key.starts_with("Software\\QuotaRail\\Tests\\")
            && key.len() < 256
        {
            return Self { key };
        }
        Self {
            key: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
        }
    }
}
impl StartupEntry {
    pub fn read(&self) -> io::Result<Option<String>> {
        let mut size = 0;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(&self.key).as_ptr(),
                wide("QuotaRail").as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut size,
            )
        };
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        checked(result)?;
        if !(2..=32768).contains(&size) || size % 2 != 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let mut value = vec![0u16; size as usize / 2];
        checked(unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide(&self.key).as_ptr(),
                wide("QuotaRail").as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                value.as_mut_ptr().cast(),
                &mut size,
            )
        })?;
        value.truncate(size as usize / 2);
        if value.pop() != Some(0) {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        String::from_utf16(&value)
            .map(Some)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
    }
    pub fn write(&self, command: Option<&str>) -> io::Result<()> {
        let mut handle = null_mut();
        let result = unsafe {
            if command.is_some() {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(&self.key).as_ptr(),
                    0,
                    null(),
                    0,
                    KEY_SET_VALUE,
                    null(),
                    &mut handle,
                    null_mut(),
                )
            } else {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    wide(&self.key).as_ptr(),
                    0,
                    KEY_SET_VALUE,
                    &mut handle,
                )
            }
        };
        if result == ERROR_FILE_NOT_FOUND && command.is_none() {
            return Ok(());
        }
        checked(result)?;
        let key = Key(handle);
        if let Some(command) = command {
            let value = wide(command);
            checked(unsafe {
                RegSetValueExW(
                    key.0,
                    wide("QuotaRail").as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            })
        } else {
            let result = unsafe { RegDeleteValueW(key.0, wide("QuotaRail").as_ptr()) };
            if result == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                checked(result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_user_entry_is_idempotent_and_never_touches_real_run_key() {
        let entry = StartupEntry {
            key: format!("Software\\QuotaRail\\Tests\\Startup-{}", std::process::id()),
        };
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    RegDeleteKeyW(HKEY_CURRENT_USER, wide(&self.0).as_ptr());
                }
            }
        }
        let _cleanup = Cleanup(entry.key.clone());
        let value = command(
            Path::new("C:\\SYNTHETIC 程序\\QuotaRail.exe"),
            Path::new("C:\\SYNTHETIC DATA\\"),
        )
        .unwrap();
        let mut argc = 0;
        let argv = unsafe {
            windows_sys::Win32::UI::Shell::CommandLineToArgvW(wide(&value).as_ptr(), &mut argc)
        };
        assert!(!argv.is_null());
        let args: Vec<String> = (0..argc as usize)
            .map(|i| unsafe {
                let start = *argv.add(i);
                let mut len = 0;
                while *start.add(len) != 0 {
                    len += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(start, len))
            })
            .collect();
        unsafe {
            LocalFree(argv.cast());
        }
        assert_eq!(
            args,
            [
                "C:\\SYNTHETIC 程序\\QuotaRail.exe",
                "--data-dir",
                "C:\\SYNTHETIC DATA\\"
            ]
        );
        entry.write(None).unwrap();
        assert!(entry.read().unwrap().is_none());
        entry.write(Some(&value)).unwrap();
        let mut handle = null_mut();
        checked(unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&entry.key).as_ptr(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                &mut handle,
            )
        })
        .unwrap();
        let handle = Key(handle);
        let sentinel = wide("SYNTHETIC_OTHER_APP");
        checked(unsafe {
            RegSetValueExW(
                handle.0,
                wide("OtherApp").as_ptr(),
                0,
                REG_SZ,
                sentinel.as_ptr().cast(),
                (sentinel.len() * 2) as u32,
            )
        })
        .unwrap();
        entry.write(Some(&value)).unwrap();
        assert_eq!(entry.read().unwrap().as_deref(), Some(value.as_str()));
        assert!(value.ends_with("DATA\\\\\""));
        entry.write(None).unwrap();
        entry.write(None).unwrap();
        assert!(entry.read().unwrap().is_none());
        let mut bytes = 0;
        assert_eq!(
            unsafe {
                RegGetValueW(
                    handle.0,
                    null(),
                    wide("OtherApp").as_ptr(),
                    RRF_RT_REG_SZ,
                    null_mut(),
                    null_mut(),
                    &mut bytes,
                )
            },
            ERROR_SUCCESS
        );
    }
}
