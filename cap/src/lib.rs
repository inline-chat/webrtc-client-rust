mod audio;
pub mod capturer;
pub mod decoder;
mod error;
mod h264;
pub mod sharable;
mod utils;
mod video;

pub use error::Error;
pub use utils::has_permission;
pub use utils::is_supported;

pub use cidre;
pub use cidre::arc::R;
pub use cidre::cf;
pub use cidre::ci;
pub use cidre::cm;
pub use cidre::cv;
pub use cidre::ns::Id;
pub use cidre::objc;
pub use cidre::sc;
pub use cidre::vt;
