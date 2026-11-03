//! "Start with Windows" / login autostart for the tray app.

use anyhow::Result;

#[cfg(windows)]
mod imp {
    use crate::win::wide;
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Ember";

    pub fn is_enabled() -> bool {
        let (key, val) = (wide(RUN_KEY), wide(VALUE));
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        r == ERROR_SUCCESS
    }

    pub fn set_enabled(on: bool) -> Result<()> {
        let (key, val) = (wide(RUN_KEY), wide(VALUE));
        let r = if on {
            let exe = std::env::current_exe()?;
            let cmd = format!("\"{}\"", exe.display());
            let data = wide(&cmd);
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    val.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr()) }
        };
        if r != ERROR_SUCCESS {
            bail!("updating registry Run key failed (error {r})");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;
    use std::path::PathBuf;

    fn desktop_file() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.config_dir().join("autostart").join("ember.desktop"))
    }

    pub fn is_enabled() -> bool {
        desktop_file().is_some_and(|p| p.exists())
    }

    pub fn set_enabled(on: bool) -> Result<()> {
        let path = desktop_file().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if on {
            let exe = std::env::current_exe()?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(
                &path,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Ember\nComment=Screen colour temperature\n\
                     Exec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
                    exe.display()
                ),
            )?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

pub fn is_enabled() -> bool {
    imp::is_enabled()
}

pub fn set_enabled(on: bool) -> Result<()> {
    imp::set_enabled(on)
}

/// If autostart is on, point it at this exe. A new release is a new file (often somewhere else),
/// so running it once is then enough for it to start at login instead of the old one.
pub fn follow_current_exe() -> Result<()> {
    if is_enabled() {
        set_enabled(true)?;
    }
    Ok(())
}
