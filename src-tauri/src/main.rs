// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  // Redirect stderr to a log file in release builds so timing data is captured
  // even when launched normally. View with: cat /tmp/quickshotter.log
  // Debug builds print to terminal as usual.
  #[cfg(all(target_os = "macos", not(debug_assertions)))]
  {
    let log_path = std::env::temp_dir().join("quickshotter.log");
    if let Ok(file) = std::fs::File::create(&log_path) {
      use std::os::unix::io::{IntoRawFd, RawFd};
      let fd: RawFd = file.into_raw_fd();
      extern "C" { fn dup2(oldfd: i32, newfd: i32) -> i32; }
      unsafe { dup2(fd, 2); }
      eprintln!("=== QuickShotter log started ===");
    }
  }

  quickshotter_lib::run()
}
