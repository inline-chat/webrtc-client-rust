use str0m::{error::IceError, RtcError};
use thiserror::Error;
// #[error("Invalid header (expected {expected:?}, got {found:?})")]
//  InvalidHeader {
//      expected: String,
//      found: String,
//  },
//  #[error("Missing attribute: {0}")]
//  MissingAttribute(String),

#[derive(Error, Debug)]
pub enum EngineError {
  /// ICE errors
  #[error("No network interface found for connection")]
  IceNoNetworkInterface,
  #[error("No stun found for interface")]
  IceStunForInterface,

  /// ICE agent errors.
  #[error("{0}")]
  Ice(#[from] IceError),

  /// ICE agent errors.
  #[error("{0}")]
  Rtc(#[from] RtcError),

  /// STUN error from webrtc-rs
  #[error("WebRTC utility error: {0}")]
  WebRtcUtil(#[from] webrtc::util::Error),

  /// STUN error from webrtc-rs
  #[error("STUN error: {0}")]
  Stun(#[from] webrtc::stun::Error),

  /// TURN error from webrtc-rs
  #[error("TURN error: {0}")]
  Turn(#[from] webrtc::turn::Error),

  /// Other IO errors.
  #[error("{0}")]
  Io(#[from] std::io::Error),
  // #[error("{0}")]
  // Other(String),
}
