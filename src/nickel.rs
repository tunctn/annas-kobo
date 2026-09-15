//! Talking to Nickel, the Kobo's reader UI. Nickel only learns about new
//! files when it rescans the library. From outside Nickel there are two
//! ways: NickelDBus (`qndb`, a separate install) or a NickelMenu entry
//! (`nickel_misc:rescan_books`), which the user taps. The old trick of
//! faking a USB plug through /tmp/nickel-hardware-status only shows the
//! "connect?" dialog on current firmware and imports nothing.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

const QNDB: &str = "/usr/bin/qndb";

/// True on a Kobo (Nickel's hardware FIFO exists).
pub fn on_kobo() -> bool {
    Path::new("/tmp/nickel-hardware-status").exists()
}

/// True when NickelDBus is installed, so imports can be automatic.
pub fn available() -> bool {
    Path::new(QNDB).exists()
}

pub const MANUAL_HINT: &str = "connect the Kobo to a computer and unplug it, add a NickelMenu entry `menu_item:main:Import books:nickel_misc:rescan_books`, or install NickelDBus for automatic import";

/// Asks Nickel to rescan the library. Blocks until Nickel answers.
pub fn import() -> Result<(), String> {
    if !on_kobo() {
        return Err("not running on a Kobo".into());
    }
    if !available() {
        return Err(format!("automatic import needs NickelDBus; {MANUAL_HINT}"));
    }
    // Everything must be on disk before Nickel scans the FAT partition.
    Command::new("sync").status().ok();
    log::info!("asking Nickel to rescan the library (qndb pfmRescanBooks)");
    let out = Command::new(QNDB)
        .args(["-t", "60000", "-m", "pfmRescanBooks"])
        .output()
        .map_err(|e| format!("run qndb: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "qndb failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}
