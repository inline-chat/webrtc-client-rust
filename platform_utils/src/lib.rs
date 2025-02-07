extern crate core_graphics;

mod macos;

pub fn get_idle_time() -> f64 {
  #[cfg(target_os = "macos")]
  return macos::idle_time::get_idle_time();

  #[cfg(target_os = "windows")]
  return 0.0;

  #[cfg(target_os = "linux")]
  return 0.0;
}

pub fn key_pressed() -> bool {
  #[cfg(target_os = "macos")]
  return macos::key_pressed();

  #[cfg(target_os = "windows")]
  return false;

  #[cfg(target_os = "linux")]
  return false;
}
