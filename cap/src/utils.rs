use core_graphics::access::ScreenCaptureAccess;

pub fn has_permission() -> bool {
  let access = ScreenCaptureAccess;
  access.request()
}

pub fn is_supported() -> bool {
  let os_version = sysinfo::System::os_version()
    .expect("Failed to get macOS version")
    .as_bytes()
    .to_vec();

  let min_version: Vec<u8> = "12.3\n".as_bytes().to_vec();

  os_version >= min_version
}
