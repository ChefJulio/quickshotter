/// Windows Explorer context menu registration via direct registry API.
/// Adds/removes "Annotate with QuickShotter" for image file types.
/// Uses Win32 API directly to avoid console window flash from reg.exe.

#[cfg(target_os = "windows")]
mod win {
  use std::ffi::OsStr;
  use std::os::windows::ffi::OsStrExt;
  use std::ptr;

  type HKEY = isize;
  const HKEY_CURRENT_USER: HKEY = -2147483647; // 0x80000001
  const KEY_ALL_ACCESS: u32 = 0xF003F;
  const REG_SZ: u32 = 1;
  const ERROR_SUCCESS: i32 = 0;

  #[link(name = "advapi32")]
  extern "system" {
    fn RegCreateKeyExW(
      hKey: HKEY, lpSubKey: *const u16, Reserved: u32,
      lpClass: *const u16, dwOptions: u32, samDesired: u32,
      lpSecurityAttributes: *const u8, phkResult: *mut HKEY,
      lpdwDisposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
      hKey: HKEY, lpValueName: *const u16, Reserved: u32,
      dwType: u32, lpData: *const u8, cbData: u32,
    ) -> i32;
    fn RegDeleteTreeW(hKey: HKEY, lpSubKey: *const u16) -> i32;
    fn RegCloseKey(hKey: HKEY) -> i32;
  }

  fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
  }

  const EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".bmp"];

  pub fn set_context_menu(enabled: bool) {
    let exe = match std::env::current_exe() {
      Ok(p) => p.to_string_lossy().to_string(),
      Err(e) => { eprintln!("context_menu: {e}"); return; }
    };

    let command_value = format!("\"{}\" \"--annotate\" \"%1\"", exe);

    for ext in EXTENSIONS {
      let base = format!("Software\\Classes\\SystemFileAssociations\\{}\\shell\\QuickShotter", ext);
      if enabled {
        reg_set(&base, "", "Annotate with QuickShotter");
        reg_set(&base, "Icon", &exe);
        reg_set(&format!("{}\\command", base), "", &command_value);
      } else {
        reg_delete_tree(&base);
      }
    }
  }

  fn reg_set(subkey: &str, name: &str, value: &str) {
    unsafe {
      let wide_key = to_wide(subkey);
      let mut hkey: HKEY = 0;
      let mut disp: u32 = 0;
      let ret = RegCreateKeyExW(
        HKEY_CURRENT_USER, wide_key.as_ptr(), 0,
        ptr::null(), 0, KEY_ALL_ACCESS,
        ptr::null(), &mut hkey, &mut disp,
      );
      if ret != ERROR_SUCCESS { return; }

      let wide_name = to_wide(name);
      let name_ptr = if name.is_empty() { ptr::null() } else { wide_name.as_ptr() };
      let wide_value = to_wide(value);
      let byte_len = (wide_value.len() * 2) as u32;

      RegSetValueExW(hkey, name_ptr, 0, REG_SZ, wide_value.as_ptr() as *const u8, byte_len);
      RegCloseKey(hkey);
    }
  }

  fn reg_delete_tree(subkey: &str) {
    unsafe {
      let wide_key = to_wide(subkey);
      RegDeleteTreeW(HKEY_CURRENT_USER, wide_key.as_ptr());
    }
  }
}

#[cfg(target_os = "windows")]
pub use win::set_context_menu;

#[cfg(not(target_os = "windows"))]
pub fn set_context_menu(_enabled: bool) {
  // Explorer context menu is Windows-only
}
