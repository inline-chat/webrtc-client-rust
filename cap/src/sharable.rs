use cidre::{
  arc::{self, Retain},
  ns,
  sc::Display,
};
use serde::Serialize;

pub async fn get_sharable_contents() -> anyhow::Result<Vec<SharableItem>, crate::error::Error> {
  if !crate::has_permission() {
    return Err(crate::error::Error::PermissionDenied);
  }

  // display
  let content = cidre::sc::ShareableContent::current()
    .await
    .expect("content");

  let primary_id: u32 = cidre::cg::main_display_id();
  let displays: Vec<SharableItem> = content
    .displays()
    .iter()
    .map(|display| {
      let is_primary = display.display_id() == primary_id;

      SharableItem {
        id: display.display_id().into(),
        kind: SharableKind::Display,
        title: format!(
          "{} ({w}x{h})",
          if is_primary {
            "Primary Display"
          } else {
            "Display"
          },
          w = display.width(),
          h = display.height()
        ),
      }
    })
    .collect();

  Ok(displays)
}

pub async fn get_display_by_id(display_id: u32) -> Option<arc::R<Display>> {
  // display
  let content = cidre::sc::ShareableContent::current()
    .await
    .expect("content");

  let display = content
    .displays()
    .iter()
    .find(|d| d.display_id() == display_id)
    .map(|d| d.retained());

  display
}

pub async fn get_displays() -> arc::R<ns::Array<Display>> {
  cidre::sc::ShareableContent::current()
    .await
    .expect("content")
    .displays()
    .retained()
}

#[derive(Debug, Serialize)]
pub enum SharableKind {
  Window,
  Display,
  App,
}

#[derive(Debug, Serialize)]
pub struct SharableItem {
  pub kind: SharableKind,
  pub title: String,

  /// id for display and window
  pub id: i64,
}

unsafe impl Send for SharableItem {}

// impl From<RunningApp> for SharableItem {
//   fn from(app: RunningApp) -> Self {
//     Self {
//       id: app.process_id().into(),
//       kind: SharableKind::App,
//       title: app.app_name().to_string(),
//     }
//   }
// }

// impl From<&Display> for SharableItem {
//   fn from(display: &Display) -> Self {
//     let primary_id: u32 = cidre::cg::main_display_id().into();
//     let is_primary = display.display_id() == primary_id;

//     Self {
//       id: display.display_id().into(),
//       kind: SharableKind::Display,
//       title: format!(
//         "{} ({w}x{h})",
//         if is_primary {
//           "Primary Display"
//         } else {
//           "Display"
//         },
//         w = display.width(),
//         h = display.height()
//       ),
//     }
//   }
// }

// impl From<Window> for SharableItem {
//   fn from(window: Window) -> Self {
//     let title = window
//       .title()
//       .map(|t| t.to_string())
//       .unwrap_or(String::from("_"));
//     Self {
//       id: window.id().into(),
//       kind: SharableKind::Window,
//       title: format!(
//         "{title} ({app})",
//         app = window
//           .owning_app()
//           .map(|app| app.app_name().to_string())
//           .unwrap_or(String::from("_"))
//       ),
//     }
//   }
// }
