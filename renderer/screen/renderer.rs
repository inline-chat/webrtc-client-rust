use crate::rtc::utils::UserId;
use crate::window_ext::WindowExt;
use cap::decoder::Decoder;
use cap::decoder::DecoderOutput;
use cocoa::quartzcore::transaction;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use std::sync::Arc;
use str0m::media::MediaData;
use tauri::window::EffectsBuilder;
use tauri::AppHandle;
use tauri::LogicalSize;
use tauri::Manager;
use tauri::PhysicalPosition;
use tauri::PhysicalSize;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;

/// Managing screen window  and render
pub struct ScreenManager {
  command_sender: flume::Sender<ScreenCommand>,
}

/// Manage video renderers
impl ScreenManager {
  pub fn new(app: &tauri::AppHandle) -> Self {
    let mut run_loop = ScreenWindowRunLoop::new(app.clone());
    let command_sender = run_loop.sender().clone();

    tokio::spawn(async move {
      run_loop.run().await;
    });

    Self { command_sender }
  }

  pub fn add_media_data(&mut self, user_id: UserId, media_data: Arc<MediaData>) {
    let _ = self
      .command_sender
      .try_send(ScreenCommand::MediaData(user_id, media_data));
  }

  /// Create viewer window and show it
  pub fn create_window(&self) {
    let _ = self.command_sender.try_send(ScreenCommand::CreateWindow);
  }

  /// Switch viewing screen
  pub fn set_active_screen(&self, user_id: UserId) {
    let _ = self
      .command_sender
      .try_send(ScreenCommand::SetActiveUser(user_id));
  }
}

impl Drop for ScreenManager {
  fn drop(&mut self) {
    let _ = self.command_sender.try_send(ScreenCommand::Destroy);
  }
}

use cocoa::quartzcore::CALayer;

use cocoa::{
  appkit::NSView,
  base::nil,
  foundation::{NSPoint, NSRect, NSSize},
};
use objc::{msg_send, runtime::Object, sel, sel_impl};
use std::ffi::c_void;

use super::cmds::emit_video_size;
use super::cmds::VideoSizePayload;

const TOOLBAR_HEIGHT: f64 = 38.0;

pub fn add_overlay_to_window(
  window: &tauri::WebviewWindow,
  initial_size: &PhysicalSize<u32>,
) -> ScreenOverlayContext {
  let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
    unreachable!("only runs on macos")
  };

  unsafe {
    // let ns_window = handle.ns_window as *mut Object;
    // let content_view: *mut Object = msg_send![ns_window, contentView];
    // let ns_window = handle.ns_window as *mut Object;
    let content_view: *mut Object = handle.ns_view.as_ptr() as *mut Object;

    // Make a new layer
    let lay = CALayer::new();
    lay.remove_all_animation();
    let lay_id = lay.id();

    // Make a new view
    let new_view = NSView::alloc(nil).initWithFrame_(NSRect::new(
      NSPoint::new(0.0, 0.0),
      NSSize::new(
        initial_size.width as f64,
        initial_size.height as f64 - TOOLBAR_HEIGHT,
      ),
    ));
    new_view.setWantsLayer(cocoa::base::YES);

    // Set layer
    new_view.setLayer(lay_id);
    new_view.setLayerContentsPlacement(cocoa::appkit::NSViewLayerContentsPlacement::NSViewLayerContentsPlacementScaleProportionallyToFit);

    // Add it to the contentView, as a sibling of webview, so that it appears on top
    let _: c_void = msg_send![content_view, addSubview: new_view];

    ScreenOverlayContext::new(window.clone(), new_view, lay)
  }
}

pub enum ScreenCommand {
  // Render(cap::R<cap::cv::ImageBuf>),
  CreateWindow,
  WindowClosed,
  SetActiveUser(UserId),
  ContextCreated(Arc<ScreenOverlayContext>),
  MediaData(UserId, Arc<MediaData>),
  Destroy,
}

pub struct ScreenWindowRunLoop {
  handle: AppHandle,
  active_ctx: Option<Arc<ScreenOverlayContext>>,
  command_receiver: flume::Receiver<ScreenCommand>,
  command_sender: flume::Sender<ScreenCommand>,

  decoder: Option<Decoder>,
  decoder_sender: flume::Sender<DecoderOutput>,
  decoder_receiver: flume::Receiver<DecoderOutput>,

  // state
  active_user_id: Option<UserId>,
  size: Option<cap::cidre::cg::Size>,

  frame_no: usize,
}

impl ScreenWindowRunLoop {
  pub fn new(handle: AppHandle) -> Self {
    let (command_sender, command_receiver) = flume::bounded(1000);
    let (decoder_sender, decoder_receiver) = flume::bounded::<DecoderOutput>(1000);

    Self {
      handle,
      active_ctx: None,
      command_receiver,
      command_sender,
      decoder: None,
      decoder_sender,
      decoder_receiver,
      active_user_id: None,
      size: None,
      frame_no: 0,
    }
  }

  /// Get command sender to control the run loop
  pub fn sender(&self) -> &flume::Sender<ScreenCommand> {
    &self.command_sender
  }

  /// Run the main loop inside its task
  pub async fn run(&mut self) {
    // Setup your env here

    loop {
      tokio::select! {
        // handle player commands
        Ok(command) = self.command_receiver.recv_async() => {
          match command {
            ScreenCommand::Destroy => {
              self.teardown(true);

              // End operation and run loop
              break;
            }
            _ => {}
          }

          self.handle_command(command).await;
        }

        // Render event (decoder sends response here)
        Ok(frame) = self.decoder_receiver.recv_async() => {
          self.render(frame);
        }
      }
    }

    // Clean up here
    info!("Screen window run loop ended.")
  }

  /// Render frame onto context
  fn render(&mut self, frame: DecoderOutput) {
    if let Some(ctx) = self.active_ctx.as_ref() {
      self.frame_no = self.frame_no + 1;

      // Save size
      let size = frame.image_buf.display_size();
      if self.size.is_none() {
        // resize
        let width: f64 = 900.0;
        let ratio = size.width / size.height;
        self.active_ctx.as_ref().map(|ctx| {
          ctx
            .window
            .set_size(LogicalSize::new(width, (width / ratio) + TOOLBAR_HEIGHT))
        });

        self.size = Some(size);

        // emit initially
        // self.active_ctx.as_ref().map(|ctx| {
        //   emit_video_size(
        //     &ctx.window,
        //     VideoSizePayload {
        //       width: size.width,
        //       height: size.height,
        //     },
        //   )
        // });
      }

      // emit peridically
      // todo: optimize to emit only on size change
      if let Some(size) = self.size {
        if self.frame_no % 10 == 0 {
          self.active_ctx.as_ref().map(|ctx| {
            emit_video_size(
              &ctx.window,
              VideoSizePayload {
                width: size.width,
                height: size.height,
              },
            )
          });
        }
      }

      // Render
      ctx.render(frame.image_buf);
    }
  }

  async fn handle_command(&mut self, command: ScreenCommand) {
    match command {
      // todo: Add user id
      ScreenCommand::CreateWindow => {
        // Create decoder
        let decoder = Decoder::new(self.decoder_sender.clone());
        self.decoder = Some(decoder);

        // Drop previous context
        self.create_window();
      }

      ScreenCommand::ContextCreated(ctx) => {
        self.active_ctx = Some(ctx);
      }

      ScreenCommand::SetActiveUser(user_id) => {
        // recreate decoder
        self.active_user_id = Some(user_id);
      }

      ScreenCommand::MediaData(user_id, media_data) => {
        let Some(active_user_id) = self.active_user_id.as_ref() else {
          // no active user id
          return;
        };

        if active_user_id != &user_id {
          // not currently viewing
          return;
        }

        self
          .decoder
          .as_mut()
          .map(|decoder| decoder.decode(media_data.data.as_slice()));
        // response will come through our decoder_receiver;
      }

      ScreenCommand::Destroy => {
        // handled above
      }

      ScreenCommand::WindowClosed => {
        self.teardown(false);
      }
    }
  }

  fn teardown(&mut self, destroy_window: bool) {
    // Drop context
    let _ = self.decoder.take();
    let _ = self.active_user_id.take();
    if let Some(ctx) = self.active_ctx.take() {
      if destroy_window {
        let _ = ctx.window.destroy();
      }
      // let win = ctx.window.clone();
      // let _ = win.close();
      // drop(ctx);
    }
  }

  /// Must run on main thread
  fn create_window(&mut self) {
    let size = PhysicalSize::new(900, 506 + TOOLBAR_HEIGHT as u32);

    if let Some(existing) = self.handle.get_webview_window("screen") {
      let _ = existing.show();
      let _ = existing.set_focus();
      return;
    }

    // Create window
    let window = WebviewWindowBuilder::new(
      &self.handle,
      "screen",
      WebviewUrl::App("screen.html".into()),
    )
    .inner_size(size.width as f64, size.height as f64)
    .title_bar_style(tauri::TitleBarStyle::Overlay)
    .effects(
      EffectsBuilder::new()
        .effect(tauri::window::Effect::HeaderView)
        .state(tauri::window::EffectState::FollowsWindowActiveState)
        .radius(9.0)
        .build(),
    )
    .build()
    .expect("to build screen window");

    // Show window
    // window.show().expect("to show screen window");
    info!("created window");

    let window_ = window.clone();
    let _window__ = window.clone();
    let sender__ = self.command_sender.clone();

    let _ = window.run_on_main_thread(move || {
      // ...
      // add overlay
      let ctx = Arc::new(add_overlay_to_window(&window_, &size));
      let weak_ctx = Arc::downgrade(&ctx);

      // save on app_handle
      let _ = sender__.try_send(ScreenCommand::ContextCreated(ctx));

      // style window
      #[cfg(target_os = "macos")]
      window_.set_transparent_titlebar();
      #[cfg(target_os = "macos")]
      window_.set_background(0.15, 0.19, 0.24);

      let window__ = window_.clone();
      window_.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_size) => {
          // Using `size` from the event broke on tauri beta 1
          let size = window__.outer_size().expect("to have inner size");
          println!("resized {:?}", &size);
          if let Some(ctx) = weak_ctx.upgrade() {
            let new_size =
              size.to_logical::<u32>(window__.scale_factor().expect("to have scale factor"));
            ctx.set_size(&new_size);
          }
        }

        tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
          if let Some(ctx) = weak_ctx.upgrade() {
            // Using `inner_size` broke on tauri beta 1
            let new_outer_size = window__.outer_size().expect("to have inner size");
            let new_size = new_outer_size.to_logical::<u32>(*scale_factor);
            ctx.set_size(&new_size);
          }
        }

        tauri::WindowEvent::CloseRequested { .. } => {
          let _ = sender__.try_send(ScreenCommand::WindowClosed);
        }

        _ => {}
      });
    });
  }
}

impl Drop for ScreenWindowRunLoop {
  fn drop(&mut self) {
    self.teardown(true);
  }
}

pub struct ScreenOverlayContext {
  window: tauri::WebviewWindow,
  // ns_window: *mut Object,
  ns_view: *mut Object,
  pub layer: CALayer,
  // ns_window: Id<NSWindow>,
  // ns_view: Id<NSView>,
  // pub layer: CALayer,
}

impl ScreenOverlayContext {
  pub fn new(
    window: tauri::WebviewWindow,
    // ns_window: *mut Object,
    ns_view: *mut Object,
    layer: CALayer,
    // ns_window: Id<NSWindow>,
    // ns_view: Id<NSView>,
    // layer: CALayer,
  ) -> Self {
    // ns_view.retain();
    // ns_window.retain();
    Self {
      window,
      // ns_window: ns_window,
      ns_view: ns_view,
      layer,
    }
  }

  /// Must be run on main thread
  pub fn set_size(&self, size: &LogicalSize<u32>) {
    print!("set_size");
    let size = NSSize::new(size.width as f64, size.height as f64 - TOOLBAR_HEIGHT);
    unsafe { self.ns_view.setFrameSize(size) };
  }

  /// Must be run on main thread
  pub fn set_position(&self, _point: &PhysicalPosition<u32>) {}

  pub fn render(&self, buf: cap::R<cap::cv::ImageBuf>) {
    print!("render");
    transaction::begin();
    transaction::set_disable_actions(true);

    // do render
    unsafe {
      let buf_obj = buf.as_type_ptr().cast_mut() as *mut _ as *mut objc::runtime::Object;
      self.layer.set_contents(buf_obj);
    }
    // done with render

    transaction::commit();
  }

  // todo: frameSize
  // todo: frameOrigin
}

impl Drop for ScreenOverlayContext {
  fn drop(&mut self) {
    unsafe {
      // todo: release
    }
  }
}

unsafe impl Send for ScreenOverlayContext {}
unsafe impl Sync for ScreenOverlayContext {}
