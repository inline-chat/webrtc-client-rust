use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use str0m::{
  bwe::Bitrate,
  media::{MediaData, MediaKind, MediaTime},
  Candidate, IceConnectionState,
};

use crate::rtc::{
  commands::ConnectionState,
  peer2::{
    utils::{CandidateJsonStr, SdpJson, SdpType},
    BweCfg, MediaExtra, MediaType, PeerId,
  },
  utils::{CallId, UserId},
};

#[derive(Debug)]
pub enum Sdp {
  Offer(str0m::change::SdpOffer),
  Answer(str0m::change::SdpAnswer),
}

pub type PoolBytesVec = Bytes;

#[derive(Clone, Copy)]
pub struct StartScreenOpts {
  pub display_id: Option<u32>,
}

pub enum PeerChannelCommand {
  // action
  AddPeer {
    user_id: UserId,
    call_id: CallId,
    initial_offerer: bool,
    // remote_offer: Option<String>,
  },
  RemovePeer {
    user_id: UserId,
    call_id: CallId,
  },
  Join(CallId, bool),
  Leave,
  ToggleScreen,
  StartScreen(StartScreenOpts),
  ScreenKeyFrameRequest,
  StopScreen,
  OpenScreenWindow,
  OpenScreenWindowFor(UserId),
  SendDataString(UserId, String),

  ConfigBwe(BweCfg),
  SetScreenBitrate(Bitrate),

  SendSignal {
    user_id: UserId,
    signal: Signal,
  },
  SetIncomingSignal {
    user_id: UserId,
    call_id: CallId,
    signal: Signal,
  },

  SendAudio {
    data: Bytes,
    duration: MediaTime,
  },
  SendMedia {
    kind: MediaKind,
    data: PoolBytesVec,
    rtp_time: MediaTime,
    extra: Option<MediaExtra>,
  },

  /// deprectated
  EnableMic,
  /// deprectated
  DisableMic,

  ChangeMic(String),
  PeerStateChange {
    user_id: UserId,
    peer_id: PeerId,
    state: ConnectionState,
    ice_state: Option<IceConnectionState>,
  },

  // incoming
  MediaAdded {
    user_id: UserId,
    media: AudioMedia,
  },
  IncomingMediaData {
    user_id: UserId,
    data: Arc<MediaData>,
    media_type: MediaType,
  },

  // --------------------
  // internal
  // --------------------

  // emitted when we detect that a peer was disconnected, but we will re-evaluate this in engine to check if it's still valid or not
  // we set multiple timers to check if the peer is still connected
  MaybeReconnect {
    user_id: UserId,
    peer_id: PeerId,
    call_id: CallId,
    case: ReconnectCase,
  },
}

#[derive(Debug)]
pub struct AudioMedia {
  pub clock_rate: u32,
  pub channels: u16,
}

#[derive(Debug)]
pub enum ReconnectCase {
  DidNotGetFirstSignal,
  InitialFailure,
  TryIceRestart,
  Disconnect,
}

#[derive(Debug)]
pub enum Signal {
  Sdp(Sdp),
  Candidate(Candidate),
}

#[derive(Serialize, Deserialize)]
struct JsonCandidate {
  #[serde(rename = "type")]
  _type: String,
  candidate: String,
}

#[derive(Serialize, Deserialize)]
struct JsonSdp {
  #[serde(rename = "type")]
  _type: String,
  sdp: SdpJson,
  // sdp: String,
}

impl Signal {
  pub fn is_sdp(&self) -> bool {
    matches!(self, Self::Sdp(_))
  }

  pub fn is_candidate(&self) -> bool {
    matches!(self, Self::Candidate(_))
  }

  pub fn is_sdp_offer(&self) -> bool {
    matches!(self, Self::Sdp(Sdp::Offer(_)))
  }

  pub fn is_sdp_answer(&self) -> bool {
    matches!(self, Self::Sdp(Sdp::Answer(_)))
  }

  // consume it
  pub fn to_json(&self) -> String {
    // The type is `serde_json::Value`
    match *self {
      Self::Sdp(ref sdp) => serde_json::to_string(&JsonSdp {
        _type: "sdp".into(),
        sdp: match sdp {
          Sdp::Answer(answer) => SdpJson {
            sdp_type: SdpType::Answer,
            sdp_string: answer.to_sdp_string(),
          },
          Sdp::Offer(offer) => SdpJson {
            sdp_type: SdpType::Offer,
            sdp_string: offer.to_sdp_string(),
          },
        },
        // sdp: match sdp {
        //   Sdp::Answer(answer) => answer.to_sdp_string(),
        //   Sdp::Offer(offer) => offer.to_sdp_string(),
        // },
      })
      .expect("Failed to parse SDP string"),
      Self::Candidate(ref candidate) => serde_json::to_string(&JsonCandidate {
        _type: "candidate".into(),
        candidate: serde_json::to_string(&CandidateJsonStr {
          candidate: candidate.to_string().replace("a=", ""),
          username_fragment: None,
          // username_fragment: candidate.ufrag().map(|u| u.to_string()),
          // todo
          sdp_m_line_index: Some(0),
          sdp_mid: None,
        })
        .expect("to make json candidate"),
      })
      .expect("Failed to parse candidate string"),
    }
  }
}
