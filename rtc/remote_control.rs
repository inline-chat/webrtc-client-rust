use crate::screen::cmds::{emit_data_event, OverlayDataChannelEvent};
use crate::window::create_or_get_overlay_window;

use super::peer::channel::StartScreenOpts;
use super::{peer2::DataChannelEvent, utils::UserId};

use std::sync::{Arc, Mutex};
use tauri::{
  AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

type Event = DataChannelEvent;

pub enum Command {
  // CreateOverlay,
  Event(UserId, Event),
  Start(StartScreenOpts),
  Stop,

  Destroy,
}

#[derive(Clone)]
pub struct RemoteControl {
  sender: flume::Sender<Command>,
}

impl RemoteControl {
  pub fn new(app_handle: &AppHandle) -> Self {
    let (sender, receiver) = flume::bounded(1_000);
    let mut run_loop = RemoteControlRunLoop::new(receiver, &app_handle);

    tokio::spawn(async move {
      run_loop.run().await;
      error!("remote control run loop exited");
    });

    Self { sender }
  }

  pub fn event(&self, user_id: UserId, event: Event) {
    let _ = self.sender.try_send(Command::Event(user_id, event));
  }

  pub fn start(&self, opts: StartScreenOpts) {
    let _ = self.sender.try_send(Command::Start(opts));
  }

  pub fn stop(&self) {
    let _ = self.sender.try_send(Command::Stop);
  }

  pub fn destroy(&self) {
    let _ = self.sender.try_send(Command::Destroy);
  }
}

impl Drop for RemoteControl {
  fn drop(&mut self) {
    self.destroy();
  }
}

enum Controller {
  Us,
  Peer(UserId),
}

pub struct RemoteControlRunLoop {
  overlay: Option<WebviewWindow>,

  /// Who is in control
  controller: Controller,

  /// Command receiver
  receiver: flume::Receiver<Command>,

  app_handle: AppHandle,

  display_size: Option<PhysicalSize<f64>>,
  display_position: Option<PhysicalPosition<f64>>,
  // enigo: Arc<Mutex<Enigo>>,
}

impl Drop for RemoteControlRunLoop {
  fn drop(&mut self) {
    self.handle_stop()
  }
}

impl RemoteControlRunLoop {
  pub fn new(receiver: flume::Receiver<Command>, app_handle: &AppHandle) -> Self {
    // let enigo = Enigo::new();
    Self {
      overlay: None,
      display_size: None,
      display_position: None,
      controller: Controller::Us,
      receiver,
      app_handle: app_handle.clone(),
      // enigo: Arc::new(Mutex::new(enigo)),
    }
  }

  pub async fn run(&mut self) {
    while let Ok(command) = self.receiver.recv_async().await {
      match command {
        Command::Event(user_id, event) => {
          self.handle_event(user_id, event);
        }

        Command::Start(opts) => {
          self.handle_start(opts).await;
        }

        Command::Stop => {
          self.handle_stop();
        }

        Command::Destroy => {
          self.handle_stop();
          break;
        }
      }
    }
  }

  async fn handle_start(&mut self, opts: StartScreenOpts) {
    let Some(display_id) = opts.display_id else {
      return;
    };

    // find display rect
    let Some(display) = cap::sharable::get_display_by_id(display_id).await else {
      println!("failed to get display  ");
      return;
    };

    println!("got display");

    // Save display frame
    let frame = display.frame();
    self.display_size = Some(PhysicalSize {
      width: frame.size.width,
      height: frame.size.height,
    });
    self.display_position = Some(PhysicalPosition {
      x: frame.origin.x,
      y: frame.origin.y,
    });

    // Create overlay
    let Ok(overlay) = create_or_get_overlay_window(&self.app_handle) else {
      error!("failed to create overlay");
      return;
    };
    self.overlay = Some(overlay.clone());

    let size = LogicalSize {
      width: frame.size.width,
      height: frame.size.height,
    };
    let position = LogicalPosition {
      x: frame.origin.x,
      y: frame.origin.y,
    };

    /// Manage overlay size
    if let Some(overlay_frame) = self.app_handle.try_state::<OverlayFrame>() {
      // Update values
      let _ = overlay_frame.set_size(size.clone());
      let _ = overlay_frame.set_position(position.clone());
    } else {
      // Save for first time
      let overlay_frame = OverlayFrame::new(size, position);
      let _ = self.app_handle.manage(overlay_frame);
    };

    // set size and then show
    let _ = overlay.set_size(size.clone());
    let _ = overlay.set_position(position.clone());
    let _ = overlay.show();

    let overlay_ = overlay.clone();
    let handle_ = self.app_handle.clone();

    overlay.on_window_event(move |event| match event {
      tauri::WindowEvent::Moved(position_) => {
        info!("moved {:#?}", position_);
        let Some(overlay_frame) = handle_.try_state::<OverlayFrame>() else {
          return;
        };
        let _ = overlay_.set_position(overlay_frame.get_position());
      }

      tauri::WindowEvent::Resized(size_) => {
        info!("resized {:#?}", size_);
        let Some(overlay_frame) = handle_.try_state::<OverlayFrame>() else {
          return;
        };
        let _ = overlay_.set_size(overlay_frame.get_size());
      }

      tauri::WindowEvent::Focused(focused) => {
        info!("focused {:#?}", focused);
      }
      _ => {}
    });
  }

  fn handle_stop(&mut self) {
    // close overlay
    if let Some(overlay) = self.overlay.take() {
      let _ = overlay.destroy();
    }

    // clean up
    self.display_size.take();
    self.display_position.take();
  }

  fn handle_event(&mut self, user_id: UserId, event: Event) {
    // let Some(display_size) = self.display_size else {
    //   return;
    // };
    // let Some(display_position) = self.display_position else {
    //   return;
    // };
    // let display_width = display_size.width;
    // let display_height = display_size.height;

    info!("got remote event {}", &user_id);

    // I run events here
    match event {
      DataChannelEvent::MouseMove { x: _, y: _ } => {
        // let my_x = (x * display_width) as i32;
        // let my_y = (y * display_height) as i32;

        if let Some(ref overlay) = self.overlay {
          emit_data_event(
            overlay,
            OverlayDataChannelEvent {
              id: user_id.0,
              event,
            },
          );
        }
      }
      DataChannelEvent::MouseDown {
        x: _,
        y: _,
        button: _,
      } => {
        // let my_x = (display_position.x + (x * display_width)) as i32;
        // let my_y = (display_position.y + (y * display_height)) as i32;

        // let enigo = Enigo::new();
        // self.enigo.mouse_move_to(my_x, my_y);
        // self.enigo.mouse_click(MouseButton::Left);
        // // move back
        // self.enigo.mouse_move_to(my_x, my_y);
      }

      // Unrelated
      _ => {}
    }
  }
}

struct OverlayFrame {
  size: Arc<Mutex<LogicalSize<f64>>>,
  position: Arc<Mutex<LogicalPosition<f64>>>,
}

impl OverlayFrame {
  pub fn new(size: LogicalSize<f64>, position: LogicalPosition<f64>) -> Self {
    Self {
      size: Arc::new(Mutex::new(size)),
      position: Arc::new(Mutex::new(position)),
    }
  }

  pub fn set_size(&self, input_size: LogicalSize<f64>) {
    if let Ok(mut size) = self.size.lock() {
      *size = input_size;
    }
  }

  pub fn set_position(&self, input_position: LogicalPosition<f64>) {
    if let Ok(mut position) = self.position.lock() {
      *position = input_position;
    }
  }

  pub fn get_position(&self) -> LogicalPosition<f64> {
    if let Ok(pos) = self.position.lock() {
      pos.clone()
    } else {
      LogicalPosition::new(0.0, 0.0)
    }
  }

  pub fn get_size(&self) -> LogicalSize<f64> {
    if let Ok(size) = self.size.lock() {
      size.clone()
    } else {
      LogicalSize::new(1.0, 1.0)
    }
  }
}

// struct Remote {
//   enigo: Enigo,
// }

// unsafe impl Send for Remote {}
// unsafe impl Sync for Remote {}

// impl Remote {
//   pub fn new() -> Self {
//     Self {
//       enigo: Enigo::default(),
//     }
//   }
// }
