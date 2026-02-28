/// Launch on startup -- Windows registry management.
///
/// Adds/removes QuickShotter from HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
/// No-op in dev mode (when running from a target/ directory).

#[cfg(target_os = "windows")]
pub fn set_launch_on_startup(enabled: bool) {
  use windows::core::HSTRING;
  use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
  };

  let exe_path = match std::env::current_exe() {
    Ok(p) => p,
    Err(e) => {
      eprintln!("startup: failed to get current exe path: {e}");
      return;
    }
  };

  // Skip in dev mode (running from target/ directory)
  let exe_str = exe_path.to_string_lossy();
  if exe_str.contains("\\target\\") || exe_str.contains("/target/") {
    return;
  }

  let subkey = HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Run");
  let value_name = HSTRING::from("QuickShotter");

  unsafe {
    let mut hkey = HKEY::default();
    let result = RegOpenKeyExW(HKEY_CURRENT_USER, &subkey, 0, KEY_SET_VALUE, &mut hkey);
    if result.is_err() {
      eprintln!("startup: failed to open registry key: {:?}", result);
      return;
    }

    if enabled {
      let path_wide: Vec<u16> = exe_str.encode_utf16().chain(std::iter::once(0)).collect();
      let bytes: &[u8] = std::slice::from_raw_parts(
        path_wide.as_ptr() as *const u8,
        path_wide.len() * 2,
      );
      let result = RegSetValueExW(hkey, &value_name, 0, REG_SZ, Some(bytes));
      if result.is_err() {
        eprintln!("startup: failed to set registry value: {:?}", result);
      }
    } else {
      let result = RegDeleteValueW(hkey, &value_name);
      if result.is_err() {
        // Not an error if the value doesn't exist
      }
    }

    let _ = RegCloseKey(hkey);
  }
}

#[cfg(not(target_os = "windows"))]
pub fn set_launch_on_startup(_enabled: bool) {
  // No-op on non-Windows platforms
}
