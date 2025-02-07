#[derive(thiserror::Error, Debug)]
pub enum Error {
  #[error("No permission")]
  PermissionDenied,

  #[error("Screen share requires at least macOS 12.3")]
  NotSupported,

  #[error("Unknown error occurred")]
  UnknownError,
}
