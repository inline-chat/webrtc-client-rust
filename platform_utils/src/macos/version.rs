use cocoa::base::id;
use cocoa::foundation::NSOperatingSystemVersion;

use objc::class;
use objc::msg_send;
use objc::{sel, sel_impl};

pub fn macos_version() -> (u64, u64, u64) {
  unsafe {
    let process_info: id = msg_send![class!(NSProcessInfo), processInfo];
    let version: NSOperatingSystemVersion = msg_send![process_info, operatingSystemVersion];
    (
      version.majorVersion,
      version.minorVersion,
      version.patchVersion,
    )
  }
}
