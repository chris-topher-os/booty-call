//! OS-specific boot operations: set the one-shot UEFI BootNext variable,
//! then reboot. Adding a new target OS means adding a backend here.

#[cfg_attr(target_os = "windows", allow(unused_imports))]
use anyhow::{Context, Result};
use std::process::Command;

/// Set the one-shot BootNext UEFI variable to the given boot entry number.
/// The firmware consumes it on the next boot attempt and clears it.
pub fn set_bootnext(entry: u16) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        // kernel32's SetFirmwareEnvironmentVariableW takes raw bytes, so
        // BootNext is written as a 4-byte little-endian UINT32, as the
        // UEFI spec requires.
        extern "system" {
            fn SetFirmwareEnvironmentVariableW(
                name: *const u16,
                guid: *const u16,
                value: *const u8,
                size: u32,
            ) -> i32;
            fn GetLastError() -> u32;
        }
        let name = "BootNext\0".encode_utf16().collect::<Vec<u16>>();
        let guid =
            "{8BE4DF61-93AA-11D2-AA0D-00A0C93EC6F6}\0" // EFI Global Variable namespace
                .encode_utf16()
                .collect::<Vec<u16>>();
        let value = (entry as u32).to_le_bytes();
        let ok = unsafe {
            SetFirmwareEnvironmentVariableW(name.as_ptr(), guid.as_ptr(), value.as_ptr(), 4)
        };
        anyhow::ensure!(
            ok != 0,
            "SetFirmwareEnvironmentVariableW failed: error {}", unsafe { GetLastError() }
        );
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "windows")))]
    {
        // efibootmgr parses its -n argument as hex, so pass the entry
        // number hex-encoded.
        let arg = format!("{entry:x}");
        let out = Command::new("efibootmgr")
            .args(["-n", &arg])
            .output()
            .context("running efibootmgr")?;
        anyhow::ensure!(
            out.status.success(),
            "efibootmgr -n {arg} failed: {}",
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
