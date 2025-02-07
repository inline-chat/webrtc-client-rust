use std::sync::Arc;

use crate::system::NoSleepManager;

use super::{
  engine::Engine,
  peer::channel::Signal,
  peer2::utils::{parse_candidate, parse_sdp},
  utils::{CallId, UserId},
};
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::{State, WebviewWindow};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub enum ConnectionState {
  #[serde(rename = "connected")]
  Connected,
  #[serde(rename = "connecting")]
  Connecting,
  #[serde(rename = "disconnected")]
  Disconnected,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PeerStateChangeEventPayload {
  pub user_id: UserId,
  pub call_id: CallId,
  pub state: ConnectionState,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SendSignalPayload {
  user_id: UserId,
  call_id: CallId,
  signal: String,
}

pub fn send_signal_to_win(
  window: &WebviewWindow,
  user_id: UserId,
  call_id: CallId,
  signal: Arc<Signal>,
) {
  window
    .emit(
      "rtc_send_signal",
      SendSignalPayload {
        user_id,
        call_id,
        signal: signal.to_json(),
      },
    )
    .expect("Failed to send signal");
}

/// rtc_join
#[tauri::command]
pub async fn rtc_join(
  engine: State<'_, Engine>,
  no_sleep: State<'_, NoSleepManager>,
  call_id: String,
  mic_enabled: bool,
) -> Result<(), ()> {
  engine.join_room(CallId(call_id), mic_enabled);
  no_sleep.enable();
  Ok(())
}

/// rtc_leave
#[tauri::command]
pub async fn rtc_leave(
  engine: State<'_, Engine>,
  no_sleep: State<'_, NoSleepManager>,
) -> Result<(), ()> {
  engine.leave_room();
  no_sleep.disable();
  Ok(())
}

/// rtc_enable_mic
#[tauri::command]
pub fn rtc_enable_mic(engine: State<'_, Engine>) {
  engine.toggle_mic(true);
}

/// rtc_disable_mic
#[tauri::command]
pub fn rtc_disable_mic(engine: State<'_, Engine>) {
  engine.toggle_mic(false);
}

#[tauri::command]
pub async fn rtc_send_data_string(
  engine: State<'_, Engine>,
  user_id: String,
  data: String,
) -> Result<(), ()> {
  engine.send_data_string(user_id.into(), data);
  Ok(())
}

/// rtc_set_signal (called from window)
/// --------------
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SetSignalPayload {
  user_id: String,
  call_id: String,
  sdp: Option<String>,
  candidate: Option<String>,
}
#[tauri::command]
pub async fn rtc_set_signal(
  engine: State<'_, Engine>,
  payload: SetSignalPayload,
) -> Result<(), ()> {
  let user_id = UserId(payload.user_id);
  let call_id = CallId(payload.call_id);
  let signal = if let Some(sdp) = payload.sdp {
    Signal::Sdp(parse_sdp(sdp.as_str()))
  } else if let Some(candidate_string) = payload.candidate {
    Signal::Candidate(parse_candidate(candidate_string.as_str()))
  } else {
    warn!("got empty signal");
    return Err(());
  };

  engine.set_incoming_signal(user_id, call_id, signal);
  Ok(())
}

/// Add Peer (called from window)
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AddPeerPayload {
  user_id: String,
  call_id: String,
  initial_offerer: bool,
}
#[tauri::command]
pub async fn rtc_add_peer(engine: State<'_, Engine>, payload: AddPeerPayload) -> Result<(), ()> {
  let user_id = UserId(payload.user_id);
  let call_id = CallId(payload.call_id);
  let initial_offerer = payload.initial_offerer;

  engine.add_peer(user_id, call_id, initial_offerer);
  Ok(())
}

/// Remove Peer (called from window)
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemovePeerPayload {
  user_id: String,
  call_id: String,
}
#[tauri::command]
pub async fn rtc_remove_peer(
  engine: State<'_, Engine>,
  payload: RemovePeerPayload,
) -> Result<(), ()> {
  let user_id = UserId(payload.user_id);
  let call_id = CallId(payload.call_id);
  engine.remove_peer(user_id, call_id);
  Ok(())
}

#[tauri::command]
pub fn rtc_set_echo_cancel(payload: bool) -> Result<(), ()> {
  error!("setting echo cancel no longer supported. {}", payload);
  Ok(())
}
