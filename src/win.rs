//! Small Win32 helpers shared by the library and the tray app.

use anyhow::{Result, bail};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

/// `s` as a NUL-terminated UTF-16 string for Win32 calls.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The System32 directory, e.g. `C:\Windows\System32`. Programs Ember starts from there are
/// named by full path, so a same-named exe in the current directory is never picked up.
pub fn system_dir() -> Result<PathBuf> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        bail!(
            "couldn't find the system directory: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(PathBuf::from(String::from_utf16(&buf[..n])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_dir_holds_reg_exe() {
        let sys = system_dir().unwrap();
        assert!(sys.is_absolute());
        assert!(sys.join("reg.exe").is_file());
    }
}
