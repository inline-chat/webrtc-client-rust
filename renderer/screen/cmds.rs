use cap::sharable::get_displays;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;

use crate::rtc::engine::Engine;
use crate::rtc::peer::channel::StartScreenOpts;
use crate::rtc::peer2::DataChannelEvent;
use crate::rtc::utils::UserId;

#[tauri::command]
pub async fn get_sharable_contents() -> Result<Vec<cap::sharable::SharableItem>, ()> {
  match cap::sharable::get_sharable_contents().await {
    Err(error) => {
      error!("Error getting sharable contents: {:?}", error);
      Err(())
    }
    Ok(items) => Ok(items),
  }
}

#[tauri::command]
pub async fn rtc_start_screen(
  app_handle: tauri::AppHandle,
  engine: State<'_, Engine>,
  display_id: Option<u32>,
) -> Result<(), ()> {
  // if no display, pick the one with mouse cursor in it
  if display_id.is_none() {
    let displays = get_displays().await;
    let cursor_pos = app_handle.cursor_position().map_err(|_| ())?;
    let display = displays.iter().find(|display| {
      let frame = display.frame();
      // check if cursor_pos is within the display rect
      return frame.origin.x <= cursor_pos.x
        && cursor_pos.x <= frame.origin.x + frame.size.width
        && frame.origin.y <= cursor_pos.y
        && cursor_pos.y <= frame.origin.y + frame.size.height;
    });
    if let Some(display) = display {
      engine.start_screen(StartScreenOpts {
        display_id: display.display_id().into(),
      });
      return Ok(()); // return early
    } else {
      error!("No display found for mouse position: {:?}", cursor_pos);
    }
  }

  engine.start_screen(StartScreenOpts { display_id });
  Ok(())
}

#[tauri::command]
pub async fn rtc_stop_screen(engine: State<'_, Engine>) -> Result<(), ()> {
  engine.stop_screen();
  Ok(())
}

#[tauri::command]
pub fn rtc_open_screen_window(engine: State<'_, Engine>) {
  engine.open_screen_window();
}
#[tauri::command]
pub fn rtc_open_screen_window_for(engine: State<'_, Engine>, user_id: String) {
  engine.open_screen_window_for(UserId(user_id));
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoSizePayload {
  pub width: f64,
  pub height: f64,
}

pub fn emit_video_size(window: &tauri::WebviewWindow, payload: VideoSizePayload) {
  let _ = window.emit("set_video_size", payload);
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OverlayDataChannelEvent {
  pub id: String,
  pub event: DataChannelEvent,
}

/// Used for mouse move sending to overlay
pub fn emit_data_event(window: &tauri::WebviewWindow, payload: OverlayDataChannelEvent) {
  let _ = window.emit("data_channel_event", payload);
}
