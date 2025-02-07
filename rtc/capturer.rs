use std::time::Instant;

use super::{engine::EngineSender, peer::channel::StartScreenOpts};
use crate::rtc::{peer::channel::PeerChannelCommand, peer2::BweCfg};
use cap::capturer::{CapturerOutput, ScreenCapturer};
use flume::{Receiver, Sender};
use str0m::{
  bwe::Bitrate,
  media::{MediaKind, MediaTime},
};
use tauri::AppHandle;

pub enum RunLoopEvent {
  Start(StartScreenOpts),
  Stop,
  SetBitrate(Bitrate),
  RequestKeyFrame,
  Destroy,
}

pub struct CaptureController {
  run_loop_sender: Sender<RunLoopEvent>,
}

impl CaptureController {
  pub fn new(_engine_sender: EngineSender, run_loop_sender: Sender<RunLoopEvent>) -> Self {
    Self { run_loop_sender }
  }

  pub async fn stop(&self) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::Stop);
  }

  pub async fn start(&self, opts: StartScreenOpts) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::Start(opts));
  }
  pub async fn set_bitrate(&self, bitrate: Bitrate) {
    let _ = self
      .run_loop_sender
      .try_send(RunLoopEvent::SetBitrate(bitrate));
  }

  pub async fn request_key_frame(&self) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::RequestKeyFrame);
  }
}

impl Drop for CaptureController {
  fn drop(&mut self) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::Destroy);
  }
}

pub struct CapturerRunLoop {
  reveiver: Receiver<RunLoopEvent>,
  close_receiver: Receiver<()>,
  close_sender: Sender<()>,
  engine_sender: EngineSender,
  frame_sender: Sender<CapturerOutput>,
  frame_receiver: Receiver<CapturerOutput>,
  capturer: Option<ScreenCapturer>,
  running: bool,
  start: Instant,
}

impl CapturerRunLoop {
  pub fn new(
    run_loop_reveiver: Receiver<RunLoopEvent>,
    _run_loop_sender: Sender<RunLoopEvent>,
    engine_sender: EngineSender,
    _app_handle: AppHandle,
  ) -> Self {
    let (close_sender, close_receiver) = flume::bounded::<()>(2);

    // This receives frames from our Capturer
    let (frame_sender, frame_receiver) = flume::bounded::<CapturerOutput>(100);

    Self {
      engine_sender,
      reveiver: run_loop_reveiver,
      close_receiver,
      close_sender,
      capturer: None,
      running: false,
      start: Instant::now(),
      frame_sender,
      frame_receiver,
    }
  }

  pub async fn run(&mut self) {
    loop {
      tokio::select! {
        Ok(event) = self.reveiver.recv_async() => {
          match event {
            RunLoopEvent::Destroy => {
              let _ = self.close_sender.try_send(());
              break;
            }
            _ => {}
          }
          info!("handling capturer event");
          self.handle_command(event).await;
        }

        // When an encoded frame arrives
        Ok(frame) = self.frame_receiver.recv_async() => {
          self.tick(frame).await;
        }

        _ = self.close_receiver.recv_async() => {
          break;
        }
      }
    }

    info!("RTC screen capturer run loop ended");
  }

  async fn handle_command(&mut self, event: RunLoopEvent) {
    match event {
      RunLoopEvent::Start(opts) => {
        self.start_capture(opts).await;
      }

      RunLoopEvent::SetBitrate(bitrate) => {
        self
          .capturer
          .as_mut()
          .map(|cap| cap.set_bitrate(bitrate.as_u64() as i32));
      }

      RunLoopEvent::Stop => {
        self.stop_capture().await;
      }

      RunLoopEvent::RequestKeyFrame => {
        self.capturer.as_mut().map(|cap| cap.request_keyframe());
      }

      _ => {}
    }
  }

  /// Send a frame to engine
  async fn tick(&mut self, frame: CapturerOutput) {
    if !self.running {
      return;
    }

    let rtp_time = MediaTime::from_seconds(frame.seconds);
    let _ = self.engine_sender.try_send(PeerChannelCommand::SendMedia {
      kind: MediaKind::Video,
      data: frame.data,
      rtp_time,
      extra: None,
    });
  }

  async fn start_capture(&mut self, opts: StartScreenOpts) {
    info!("starting capture");

    let mut capturer = ScreenCapturer::new(self.frame_sender.clone());

    // let mut config = capturer.config();
    if let Some(id) = opts.display_id {
      capturer.set_display(id);
    }

    let Ok(desired_bitrate) = capturer.start().await else {
      return;
    };

    // Send the desired bitrate to the engine to set on BWE for peers
    self.send_to_engine(PeerChannelCommand::ConfigBwe(BweCfg::desired(
      Bitrate::bps(desired_bitrate as u64),
    )));

    self.capturer = Some(capturer);
    self.start = Instant::now();
    self.running = true;
  }

  async fn stop_capture(&mut self) {
    info!("stopping capture");

    self.running = false;

    // stop capturing
    if let Some(mut cap) = self.capturer.take() {
      cap.stop().await;
    }
  }

  #[allow(dead_code)]
  fn send_to_engine(&mut self, command: PeerChannelCommand) {
    let _ = self.engine_sender.try_send(command);
  }
}
