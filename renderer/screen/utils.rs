// pub fn has_permission() -> bool {
//   let access = ScreenCaptureAccess::default();
//   access.request()
// }

use std::process::Command;

pub fn is_supported() -> bool {
  let min_version: Vec<u8> = "12.3\n".as_bytes().to_vec();
  let output = Command::new("sw_vers")
    .arg("-productVersion")
    .output()
    .expect("Failed to execute sw_vers command");

  let os_version = output.stdout;

  os_version >= min_version
}
