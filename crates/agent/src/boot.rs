//! OS-specific boot operations: set the one-shot UEFI BootNext variable,
//! then reboot. Adding a new target OS means adding a backend here.

use anyhow::{Context, Result};
use std::process::Command;

/// Set the one-shot BootNext UEFI variable to the given boot entry number.
/// The firmware consumes it on the next boot attempt and clears it.
pub fn set_bootnext(entry: u16) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let ps = format!(
            "Set-FirmwareEnvironmentVariable -Name 'BootNext' -Value '{entry:04x}'"
        );
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .output()
            .context("running Set-FirmwareEnvironmentVariable")?;
        anyhow::ensure!(
            out.status.success(),
            "Set-FirmwareEnvironmentVariable failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "windows")))]
    {
        let out = Command::new("efibootmgr")
            .args(["-n", &entry.to_string()])
            .output()
            .context("running efibootmgr")?;
        anyhow::ensure!(
            out.status.success(),
            "efibootmgr -n {} failed: {}",
            entry,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
}

/// Reboot the machine. Does not return on success.
pub fn reboot() -> ! {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("shutdown").args(["/r", "/t", "3", "/f"]).status();
    }
    #[cfg(all(unix, not(target_os = "windows")))]
    {
        let _ = Command::new("systemctl").arg("reboot").status();
    }
    std::process::exit(1);
}
