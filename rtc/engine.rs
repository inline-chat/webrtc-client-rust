use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  thread,
};

use super::{
  audio_input::AudioInput,
  commands::ConnectionState,
  peer::channel::{AudioMedia, PeerChannelCommand, Signal, StartScreenOpts},
  peer2::{BweCfg, PeerId},
  peer_queue::PeerQueue,
  player::PlayerController,
  processor::AudioEchoProcessor,
  signal::PendingSignals,
  utils::{CallId, UserId},
};
use crate::{
  debugger::Debugger,
  rtc::{
    audio_input::AudioInputOptions,
    capturer::{CaptureController, CapturerRunLoop},
    ice::gatherer::IcePolicy,
    peer::channel::ReconnectCase,
    peer2::{MediaType, Peer2},
    peer_state::PeerState,
    player::{Player, PlayerOptions},
    remote_control::RemoteControl,
  },
  screen::renderer::ScreenManager,
  store::SettingsStore,
};
use anyhow::anyhow;
use flume::{Receiver, Sender};
use futures::future;
use str0m::{bwe::Bitrate, change::DtlsCert, IceConnectionState};
use tauri::{async_runtime, AppHandle};
use tokio::runtime::Builder;
use tokio::time::Duration;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

type ChannelSender = Sender<PeerChannelCommand>;
type ChannelReceiver = Receiver<PeerChannelCommand>;
type EventListeners = Arc<Mutex<Vec<Box<dyn Fn(EngineEvent) + Send + Sync + 'static>>>>;
pub type EngineSender = Sender<PeerChannelCommand>;

const RECONNECT_WHEN_NO_SIGNAL: u64 = 3_000;
const RECONNECT_ON_INITIAL_FAILURE: u64 = 10_000;
const ICE_RESTART_AFTER_ICE_DISCONNECTED: u64 = 1_000;
const RECONNECT_AFTER_ICE_DISCONNECTED: u64 = 4_000;

/// Starting point of our whole webrtc engine
/// Interacts with Tauri using commands and methods `pub` from here
/// ## Notes:
/// - it should have interior mutability
/// - it should be able to be cloned
/// - it must not use app, window, etc directly (separate core)
#[derive(Clone)]
pub struct Engine {
  channel_sender: ChannelSender,
  events_listeners: EventListeners,
}

pub struct EngineOptions {
  pub debugger: Debugger,
  pub app_handle: AppHandle,
  pub settings: Arc<SettingsStore>,
}

impl Engine {
  pub fn new(options: EngineOptions) -> Self {
    let (channel_sender, channel_receiver) = flume::bounded::<PeerChannelCommand>(2000);
    let events_listeners = Arc::new(Mutex::new(Vec::new()));
    let debugger = options.debugger.clone();

    let mut engine_event_loop = EngineEventLoop::init(EngineEventLoopInput {
      channel_receiver: channel_receiver.clone(),
      channel_sender: channel_sender.clone(),
      events_listeners: events_listeners.clone(),
      debugger: options.debugger,
      settings: options.settings,
      app_handle: options.app_handle,
    });

    // thread::spawn(move || {
    async_runtime::spawn(async move {
      // move it to a thread and run it
      engine_event_loop.run().await;
      error!("engine event loop stopped");
      debugger.alert(
        "Voice Error",
        "Engine thread panicked. Please restart the app.",
      );
    });
    // error!("engine thread ended.");
    // });

    Self {
      channel_sender,
      events_listeners,
    }
  }

  fn send_command(&self, command: PeerChannelCommand) {
    if let Err(error) = self.channel_sender.try_send(command) {
      error!("failed to send command: {}", error);
    }
  }

  pub fn on_event<F: Fn(EngineEvent) + Send + Sync + 'static>(&self, handler: F) {
    info!("adding event listener");
    self
      .events_listeners
      .lock()
      .expect("poisened")
      .push(Box::new(handler));
  }

  pub fn join_room(&self, call_id: CallId, mic_enabled: bool) {
    self.send_command(PeerChannelCommand::Join(call_id, mic_enabled));
  }

  pub fn leave_room(&self) {
    self.send_command(PeerChannelCommand::Leave);
  }

  pub fn toggle_screen(&self) {
    self.send_command(PeerChannelCommand::ToggleScreen);
  }

  pub fn start_screen(&self, opts: StartScreenOpts) {
    self.send_command(PeerChannelCommand::StartScreen(opts));
  }

  pub fn stop_screen(&self) {
    self.send_command(PeerChannelCommand::StopScreen);
  }

  pub fn open_screen_window(&self) {
    self.send_command(PeerChannelCommand::OpenScreenWindow);
  }

  pub fn open_screen_window_for(&self, user_id: UserId) {
    self.send_command(PeerChannelCommand::OpenScreenWindowFor(user_id));
  }

  pub fn send_data_string(&self, user_id: UserId, json_string: String) {
    self.send_command(PeerChannelCommand::SendDataString(user_id, json_string));
  }

  pub fn add_peer(&self, user_id: UserId, call_id: CallId, initial_offerer: bool) {
    self.send_command(PeerChannelCommand::AddPeer {
      user_id,
      call_id,
      initial_offerer,
    });
  }

  pub fn remove_peer(&self, user_id: UserId, call_id: CallId) {
    self.send_command(PeerChannelCommand::RemovePeer { user_id, call_id });
  }

  pub fn toggle_mic(&self, enabled: bool) {
    if enabled {
      self.send_command(PeerChannelCommand::EnableMic);
    } else {
      self.send_command(PeerChannelCommand::DisableMic);
    }
  }

  pub fn change_mic(&self, device: String) {
    self.send_command(PeerChannelCommand::ChangeMic(device));
  }

  pub fn set_incoming_signal(&self, user_id: UserId, call_id: CallId, signal: Signal) {
    self.send_command(PeerChannelCommand::SetIncomingSignal {
      user_id,
      call_id,
      signal,
    });
  }
}

/// Triggered by engine so app can handle these and send to the frontend
#[derive(Debug, Clone)]
pub enum EngineEvent {
  ConnectionStateChange {
    user_id: UserId,
    call_id: CallId,
    state: ConnectionState,
  },
  SendSignal {
    user_id: UserId,
    call_id: CallId,
    signal: Arc<Signal>,
  },
}

struct EngineEventLoopInput {
  channel_receiver: ChannelReceiver,
  channel_sender: ChannelSender,
  events_listeners: EventListeners,
  debugger: Debugger,
  app_handle: AppHandle,
  settings: Arc<SettingsStore>,
}

/// This is run in a thread after creating the engine
/// It listens to the channel and handles the commands
/// It also emits events to the frontend
/// ## Notes:
/// - It can be &mut
/// - It should use minimum locks and mutexes
/// - It should not use app, window, etc directly (separate core)
/// - It doesn't need to be able to be cloned
struct EngineEventLoop {
  channel_receiver: ChannelReceiver,
  channel_sender: ChannelSender,
  events_listeners: EventListeners,
  debugger: Debugger,
  settings: Arc<SettingsStore>,
  app_handle: AppHandle,

  call_id: Option<CallId>,

  peers: HashMap<UserId, Arc<Peer2>>,
  peer_states: HashMap<UserId, PeerState>,
  local_tracks: HashMap<UserId, Arc<TrackLocalStaticSample>>,
  pending_signals: PendingSignals,
  peer_queue: PeerQueue,
  processor: Option<AudioEchoProcessor>,
  /// Preferred to use Weak "Peer".clone()s and check those
  peer_close_signals: HashMap<UserId, (Sender<bool>, Receiver<bool>)>,
  audio_input: Option<AudioInput>,
  player: Option<PlayerController>,
  capture_controller: Option<CaptureController>,
  screen_renderer: Option<ScreenManager>,

  // TODO: refactor into a struct
  output_runtime: Arc<tokio::runtime::Runtime>,
  input_runtime: Arc<tokio::runtime::Runtime>,
  capturer_runtime: Arc<tokio::runtime::Runtime>,
  // output_local: tokio::task::LocalSet,
  dtls_cert: DtlsCert,
  remote_control: Option<RemoteControl>,
  next_peer_id: PeerId,
}

impl EngineEventLoop {
  pub fn init(input: EngineEventLoopInput) -> Self {
    let output_runtime = Arc::new(
      Builder::new_current_thread()
        .enable_all()
        .thread_name("audio output thread")
        .build()
        .expect("to create audio output runtime"),
    );
    let input_runtime = Arc::new(
      Builder::new_current_thread()
        .enable_all()
        .thread_name("audio input thread")
        .build()
        .expect("to create audio input runtime"),
    );
    let capturer_runtime = Arc::new(
      Builder::new_current_thread()
        .enable_all()
        .thread_name("cap input thread")
        .build()
        .expect("to create cap input runtime"),
    );

    let dtls_cert = DtlsCert::new_openssl();

    Self {
      channel_receiver: input.channel_receiver,
      channel_sender: input.channel_sender,
      events_listeners: input.events_listeners,
      debugger: input.debugger,
      settings: input.settings,
      app_handle: input.app_handle,
      call_id: None,
      peer_queue: PeerQueue::init(),
      peers: HashMap::new(),
      peer_states: HashMap::new(),
      local_tracks: HashMap::new(),
      pending_signals: PendingSignals::init(),
      processor: None,
      peer_close_signals: HashMap::default(),
      audio_input: None,
      player: None,
      screen_renderer: None,
      capture_controller: None,
      remote_control: None,
      output_runtime,
      input_runtime,
      capturer_runtime,
      dtls_cert,
      next_peer_id: 0,
    }
  }

  pub async fn run(&mut self) {
    // HANDLE COMMANDS
    while let Ok(command) = self.channel_receiver.recv_async().await {
      // trace!("engine: command {:?}", &command);
      match command {
        PeerChannelCommand::Join(call_id, mic_enabled) => {
          self.join(call_id, mic_enabled);
        }

        PeerChannelCommand::Leave => {
          self.leave();
        }

        PeerChannelCommand::AddPeer {
          user_id,
          call_id,
          initial_offerer,
        } => {
          let _ = self.add_peer(user_id, initial_offerer, call_id).await;
        }

        PeerChannelCommand::RemovePeer { user_id, call_id } => {
          let _ = self.remove_peer(&user_id, call_id);
        }

        PeerChannelCommand::SetIncomingSignal {
          user_id,
          call_id,
          signal,
        } => {
          self.set_incoming_signal(user_id, call_id, signal).await;
        }

        PeerChannelCommand::EnableMic => {
          self.toggle_mic(true);
        }
        PeerChannelCommand::DisableMic => {
          self.toggle_mic(false);
        }

        PeerChannelCommand::ChangeMic(device) => {
          info!("Changing mic to {:?}", &device);
          if let Some(audio) = self.audio_input.as_ref() {
            audio.change_device(device);
          } else {
            info!("no audio input to change device yet")
          }
        }

        PeerChannelCommand::SendSignal { user_id, signal } => {
          let call_id = if let Some(call_id) = self.call_id.clone() {
            call_id
          } else {
            return;
          };

          self.emit_event(EngineEvent::SendSignal {
            user_id,
            signal: Arc::new(signal),
            call_id,
          });
        }

        PeerChannelCommand::SendAudio { data, duration } => {
          // off load it
          tokio::task::spawn(future::join_all(self.peers.values().map(|peer| {
            let data_ = data.clone();
            let peer_ = peer.clone();
            async move {
              peer_.write_audio(data_, duration).await;
            }
          })));
        }

        PeerChannelCommand::SendMedia {
          kind,
          data,

          rtp_time,
          extra,
        } => {
          // off load it
          tokio::task::spawn(future::join_all(self.peers.iter().map(move |(_, peer)| {
            let data_ = data.clone();
            let peer_ = peer.clone();
            let extra_ = extra.clone();
            async move {
              peer_.write_media(kind, data_, rtp_time, extra_).await;
            }
          })));
        }

        PeerChannelCommand::MediaAdded { user_id, media } => {
          self.got_media(user_id, media);
        }

        PeerChannelCommand::IncomingMediaData {
          data,
          user_id,
          media_type,
        } => {
          // dbg!(data.mid);
          // dbg!(data.pt);
          // mic
          // data.mid = Mid(0)
          // data.pt = Pt(111)
          // screen-share
          // data.mid = Mid(1)
          // data.pt = Pt(98)
          match media_type {
            MediaType::Voice => {
              if let Some(player) = self.player.as_ref() {
                player.add_media_data(user_id, data);
              }
            }
            MediaType::Screen => {
              if let Some(screen_renderer) = self.screen_renderer.as_mut() {
                screen_renderer.add_media_data(user_id, data);
              }
              // todo.
            }
          }
        }

        // deprecated
        PeerChannelCommand::OpenScreenWindow => {
          if let Some(screen_renderer) = self.screen_renderer.as_mut() {
            screen_renderer.create_window();
          }
        }

        PeerChannelCommand::OpenScreenWindowFor(user_id) => {
          if let Some(screen_renderer) = self.screen_renderer.as_mut() {
            screen_renderer.set_active_screen(user_id.clone());
            screen_renderer.create_window();
          }
          if let Some(peer) = self.peers.get(&user_id) {
            peer.request_screen_keyframe();
          }
        }

        PeerChannelCommand::SendDataString(user_id, data_string) => {
          if let Some(peer) = self.peers.get(&user_id) {
            peer.send_data_string(data_string);
          }
        }

        PeerChannelCommand::ScreenKeyFrameRequest => {
          if let Some(cc) = self.capture_controller.as_ref() {
            cc.request_key_frame().await;
          }
        }

        PeerChannelCommand::StartScreen(opts) => {
          self
            .config_peers_bwe(BweCfg::reset(Bitrate::kbps(900)))
            .await;
          self
            .config_peers_bwe(BweCfg::desired(Bitrate::mbps(1)))
            .await;
          self
            .config_peers_bwe(BweCfg::current(Bitrate::kbps(800)))
            .await;

          if let Some(cc) = self.capture_controller.as_ref() {
            cc.start(opts).await;
          } else {
            error!("No capture controller object found.");
          }

          if let Some(rc) = self.remote_control.as_ref() {
            rc.start(opts);
          } else {
            error!("No remote control object found.");
          }
        }

        PeerChannelCommand::StopScreen => {
          if let Some(cc) = self.capture_controller.as_ref() {
            cc.stop().await;
          }
          if let Some(rc) = self.remote_control.as_ref() {
            rc.stop();
          }

          self
            .config_peers_bwe(BweCfg::reset(Bitrate::kbps(80)))
            .await;
          self
            .config_peers_bwe(BweCfg::desired(Bitrate::kbps(80)))
            .await;
        }

        // Comes after screen-share start with desired bitrate
        PeerChannelCommand::ConfigBwe(cfg) => {
          self.config_peers_bwe(cfg).await;
        }

        // Comes from peers estimate
        PeerChannelCommand::SetScreenBitrate(bitrate) => {
          let min_bitrate = Bitrate::bps(500_000);
          let max_bitrate = Bitrate::bps(10_000_000);
          let bitrate = bitrate.max(min_bitrate).min(max_bitrate);
          if let Some(cc) = self.capture_controller.as_ref() {
            cc.set_bitrate(bitrate.clone()).await;
            self.config_peers_bwe(BweCfg::current(bitrate)).await;
          }
        }

        PeerChannelCommand::PeerStateChange {
          user_id,
          peer_id,
          state,
          ice_state,
        } => {
          self.peer_connection_state_change(&user_id, peer_id, state, ice_state);
        }

        PeerChannelCommand::MaybeReconnect {
          user_id,
          call_id,
          peer_id,
          case,
        } => {
          self.maybe_reconnect(user_id, call_id, peer_id, case).await;
        }

        _ => {}
      }
    }
  }

  async fn config_peers_bwe(&mut self, cfg: BweCfg) {
    tokio::task::spawn(future::join_all(self.peers.iter().map(move |(_, peer)| {
      let cfg_ = cfg.clone();
      let peer_ = peer.clone();
      async move {
        peer_.config_bwe(cfg_);
      }
    })));
  }

  // maybe_reconnect
  async fn maybe_reconnect(
    &mut self,
    user_id: UserId,
    call_id: CallId,
    peer_id: PeerId,
    case: ReconnectCase,
  ) {
    // Ensure still in the same call that triggered timer
    if !self.is_current_call(&call_id) {
      return;
    }

    // Ensure we still have peer
    let Some(peer) = self.peers.get(&user_id) else {
      return;
    };

    // Ensure this is the peer we started with
    if peer.id() != peer_id {
      return;
    }

    // Check status
    let Some(peer_state) = self.peer_states.get(&user_id) else {
      return;
    };

    // Cancel if we're already connected
    if *peer_state.connection_state() == ConnectionState::Connected {
      return;
    }

    let initial_offerer = peer_state.is_initial_offerer();

    match case {
      ReconnectCase::DidNotGetFirstSignal => {
        // Cancel if we have received an offer but waiting on connection to be established
        if peer_state.got_first_offer() || peer_state.got_first_answer() {
          // we could also skip this check for initial offerer? oh shit not initial offerer must start reconnect!!!
          return;
        }
        // Cancel if we have one successful connection on this peer (probably not necessary)
        if peer_state.has_connected_once() {
          return;
        }

        info!("reconnecting because initial offer/answer did not arrive");
        let _ = self.remove_peer(&user_id, call_id.to_owned());
        let _ = self.add_peer(user_id, initial_offerer, call_id).await;
      }

      ReconnectCase::InitialFailure => {
        // Cancel if we have one successful connection on this peer
        if peer_state.has_connected_once() {
          return;
        }

        info!("reconnecting because initial offer failed");
        let _ = self.remove_peer(&user_id, call_id.to_owned());
        let _ = self.add_peer(user_id, initial_offerer, call_id).await;
      }

      ReconnectCase::TryIceRestart => {
        // Cancel if ICE is not exactly disconnected
        if *peer_state.ice_connection_state() != IceConnectionState::Disconnected {
          return;
        }

        info!("restarting ice because of ice disconnect");
        peer.restart_ice();
      }
      ReconnectCase::Disconnect => {
        // Check after disconnects
        let initial_offerer = peer_state.is_initial_offerer();
        info!("reconnecting on long disconnect");
        let _ = self.remove_peer(&user_id, call_id.to_owned());
        let _ = self.add_peer(user_id, initial_offerer, call_id).await;
      }
    }
  }

  fn peer_connection_state_change(
    &mut self,
    user_id: &UserId,
    peer_id: PeerId,
    state: ConnectionState,
    ice_state: Option<IceConnectionState>,
  ) {
    let Some(call_id) = self.call_id.as_ref() else {
      return;
    };

    // Get peer id
    let Some(peer) = self.peers.get(user_id) else {
      return;
    };

    // Ignore if peer ids don't match
    // This means another peer has been created and we should ignore these events
    if peer.id() != peer_id {
      return;
    }

    // Emit to window
    self.emit_event(EngineEvent::ConnectionStateChange {
      user_id: user_id.to_owned(),
      state: state.to_owned(),
      call_id: call_id.to_owned(),
    });

    // Update engine state for peer
    let Some(peer_state) = self.peer_states.get_mut(user_id) else {
      return;
    };
    peer_state.set_connection_state(state.clone());
    if let Some(ice_state) = ice_state {
      peer_state.set_ice_connection_state(ice_state);
    }

    // Re-connect
    if ice_state == Some(IceConnectionState::Disconnected) {
      self.start_reconnect_timer(user_id, ReconnectCase::TryIceRestart);
      self.start_reconnect_timer(user_id, ReconnectCase::Disconnect);
    }
  }

  fn start_reconnect_timer(&self, user_id: &UserId, case: ReconnectCase) {
    let Some(call_id) = self.call_id.as_ref() else {
      return;
    };
    let Some(peer) = self.peers.get(user_id) else {
      return;
    };

    let peer_id = peer.id();
    let wait_ms = match case {
      ReconnectCase::DidNotGetFirstSignal => RECONNECT_WHEN_NO_SIGNAL,
      ReconnectCase::InitialFailure => RECONNECT_ON_INITIAL_FAILURE,
      ReconnectCase::TryIceRestart => ICE_RESTART_AFTER_ICE_DISCONNECTED,
      ReconnectCase::Disconnect => RECONNECT_AFTER_ICE_DISCONNECTED,
    };
    let call_id = call_id.clone();
    let user_id = user_id.clone();
    let sender = self.channel_sender.clone();

    async_runtime::spawn(async move {
      tokio::time::sleep(Duration::from_millis(wait_ms)).await;
      let _ = sender
        .send_async(PeerChannelCommand::MaybeReconnect {
          user_id,
          call_id,
          peer_id,
          case,
        })
        .await;
    });
  }

  fn join(&mut self, call_id: CallId, mic_enabled: bool) {
    if self.call_id.is_some() {
      warn!("join: already in a call");
    }
    self.leave();

    info!("PeerChannelCommand::Join {:?}", &call_id);

    // Set call id
    self.call_id = Some(call_id);

    let echo_cancel = self.settings.echo_cancel();
    let noise_cancel = self.settings.noise_cancel();

    // Create processor
    let echo_processor = AudioEchoProcessor::new(self.debugger.clone(), echo_cancel, noise_cancel);
    let echo_processor2 = echo_processor.clone();
    let echo_processor3 = echo_processor.clone();
    self.processor = Some(echo_processor);

    // Create audio input
    let mut audio_input = AudioInput::new(AudioInputOptions {
      preferred_mic: Some(self.settings.preferred_mic()),
      engine_sender: self.channel_sender.clone(),
      enabled: mic_enabled,
      echo_processor: echo_processor2,
    });
    let mut input_loop = audio_input.get_event_loop();
    // Create thread for audio input
    let debugger = self.debugger.clone();
    // self.audio_runtime.spawn(async move {
    //   let result = input_loop.run().await;
    //   if result.is_err() {
    //     debugger.alert("Mic Error", "Audio input thread exited");
    //   }
    // });

    // screen capturer
    let runtime = self.capturer_runtime.clone();
    let (run_loop_sender, run_loop_reveiver) = flume::bounded(1000);
    let run_loop_sender_ = run_loop_sender.clone();
    let engine_sender = self.channel_sender.clone();
    let app_handle_ = self.app_handle.clone();
    let capture_controller = CaptureController::new(self.channel_sender.clone(), run_loop_sender);
    self.capture_controller = Some(capture_controller);

    thread::spawn(move || {
      let capturer_local = tokio::task::LocalSet::new();

      let mut run_loop = CapturerRunLoop::new(
        run_loop_reveiver,
        run_loop_sender_,
        engine_sender,
        app_handle_,
      );

      capturer_local.spawn_local(async move {
        run_loop.run().await;
      });

      runtime.block_on(capturer_local);
      info!("Screen capture runtime thread exited");
    });

    // output
    let runtime = self.input_runtime.clone();

    thread::spawn(move || {
      let local = tokio::task::LocalSet::new();

      local.spawn_local(async move {
        let result = input_loop.run().await;
        if result.is_err() {
          debugger.alert("Mic Error", "Audio input thread exited");
        }
      });

      runtime.block_on(local);
      info!("Input runtime thread exited");
    });
    self.audio_input = Some(audio_input);

    // Create player
    let mut player = Player::new(PlayerOptions {
      preferred_device: None,
      debugger: self.debugger.clone(),
      echo_processor: echo_processor3,
    });
    // We keep the controller half, and move the player itself
    self.player = Some(player.get_controller());

    // Create thread for player
    let debugger = self.debugger.clone();
    let runtime = self.output_runtime.clone();

    thread::spawn(move || {
      let output_local = tokio::task::LocalSet::new();

      output_local.spawn_local(async move {
        let result = player.run().await;
        if result.is_err() {
          debugger.alert("Player Error", "Player thread exited");
        }
      });

      runtime.block_on(output_local);
      info!("Player runtime thread exited");
    });
    // self.audio_runtime.spawn(async move {
    //   let result = player.run().await;
    //   if result.is_err() {
    //     debugger.alert("Player Error", "Player thread exited");
    //   }
    // });

    // Renderer
    let screen_renderer = ScreenManager::new(&self.app_handle);
    self.screen_renderer = Some(screen_renderer);

    // Remote Controller
    self.remote_control = Some(RemoteControl::new(&self.app_handle));
  }

  fn leave(&mut self) {
    info!("PeerChannelCommand::Leave");

    // Stop individual peers and decoders
    for (_, (sender, _)) in self.peer_close_signals.iter() {
      let _ = sender.try_send(true);
    }

    // Remove state
    self.peers.clear();
    self.peer_states.clear();
    self.pending_signals.reset();
    self.peer_queue.clear();
    self.peer_close_signals.clear();
    self.call_id = None;

    // Stop instances
    self.audio_input = None;
    self.processor = None;
    self.player = None;
    self.capture_controller = None;
    self.screen_renderer = None;
    self.remote_control = None;
  }

  fn got_media(&mut self, user_id: UserId, media: AudioMedia) {
    if let Some(c) = self.player.as_ref() {
      c.add_media(user_id, media);
    }
  }

  fn new_peer_id(&mut self) -> PeerId {
    let id = self.next_peer_id;
    self.next_peer_id += 1;
    id
  }

  async fn add_peer(
    &mut self,
    user_id: UserId,
    initial_offerer: bool,
    call_id: CallId,
  ) -> Result<Arc<Peer2>, anyhow::Error> {
    if !self.is_current_call(&call_id) {
      return Err(anyhow!("not current call"));
    }

    let id = self.new_peer_id();

    info!(
      "add peer peer_id: {}, user_id: {}, initial_offerer: {}",
      id, &user_id, &initial_offerer
    );

    let ice_policy = self.settings.ice_policy();
    let ice_policy = match ice_policy.as_str() {
      "relay" => IcePolicy::Relay,
      "local" => IcePolicy::Local,
      "all" => IcePolicy::All,
      _ => IcePolicy::All,
    };

    let remote_control = self
      .remote_control
      .as_ref()
      .expect("to have remote control");

    // Create peer
    let peer = Peer2::new(
      id,
      user_id.clone(),
      initial_offerer,
      self.channel_sender.clone(),
      self.debugger.clone(),
      ice_policy,
      self.dtls_cert.clone(),
      remote_control.clone(),
    );

    // Add tracks and callbacks
    // let local_track = peer.add_local_tracks(48_000, 2).await?;
    // peer.set_pc_callbacks(self.debugger.clone()).await?;

    // Put it behind an arc for easier access through mutex without locking for long
    let peer = Arc::new(peer);
    let peer_clone = Arc::clone(&peer);
    let peer_clone2 = Arc::clone(&peer);
    self.peers.insert(user_id.to_owned(), peer);
    self
      .peer_states
      .insert(user_id.to_owned(), PeerState::new(initial_offerer));

    // self.local_tracks.insert(user_id.to_owned(), local_track);

    // self
    //   .audio_input
    //   .as_ref()
    //   .expect("to have audio input")
    //   .add_track(user_id.to_owned(), Arc::downgrade(&local_track));

    let peer_clone3 = Arc::downgrade(&peer_clone);
    let user_pending_signals = self.pending_signals.drain(&user_id, initial_offerer);

    let user_id_ = user_id.to_owned();
    let call_id_ = call_id.to_owned();
    let sender = self.channel_sender.clone();
    // Move here to not block the thread
    async_runtime::spawn(async move {
      // send offer
      // if initial_offerer {
      //  add a delay if we're initial offerer (?)
      //  tokio::time::sleep(Duration::from_millis(200)).await;
      //  peer_clone2.create_and_send_offer().await;
      // }

      // Only for the not initial offerer
      if initial_offerer {
        return;
      }

      const WAIT_TO_APPLY_PENDING_SIGNALS: u64 = 400;
      // Wait if no new offer was received
      tokio::time::sleep(Duration::from_millis(WAIT_TO_APPLY_PENDING_SIGNALS)).await;

      let Some(_peer) = peer_clone3.upgrade() else {
        return;
      };

      // if there are pending signals, add them
      if let Some(user_pending_signals) = user_pending_signals {
        for signal in user_pending_signals {
          info!("putting pending signal into peer {:?}", signal);
          // peer.handle_signal(signal).await;

          // Move it through thr flow so peer state gets updated correctly
          let _ = sender
            .send_async(PeerChannelCommand::SetIncomingSignal {
              user_id: user_id_.to_owned(),
              call_id: call_id_.to_owned(),
              signal,
            })
            .await;
        }
      }
    });

    // Reconnect timer on initial failure
    if initial_offerer {
      self.start_reconnect_timer(&user_id, ReconnectCase::DidNotGetFirstSignal);
      self.start_reconnect_timer(&user_id, ReconnectCase::InitialFailure);
    }

    Ok(peer_clone2)
  }

  fn remove_peer(&mut self, user_id: &UserId, call_id: CallId) -> Result<(), anyhow::Error> {
    if !self.is_current_call(&call_id) {
      return Err(anyhow!("not current call"));
    }

    info!("remove peer user_id: {}", user_id);

    // Drop peer
    let _ = self.peers.remove(user_id);
    let _ = self.peer_states.remove(user_id);

    // Drop local track
    let _ = self.local_tracks.remove(user_id);
    // self
    //   .audio_input
    //   .as_ref()
    //   .expect("to have audio input")
    //   .remove_track(user_id.to_owned());
    // Drop player remote track
    if let Some(player) = self.player.as_ref() {
      player.remove_media(user_id.to_owned());
    }

    // Remove pending signals
    self.pending_signals.remove(user_id);

    Ok(())
  }

  async fn set_incoming_signal(&mut self, user_id: UserId, call_id: CallId, signal: Signal) {
    if !self.is_current_call(&call_id) {
      error!(
        "set_incoming_signal signal called with different callId {:?}",
        &call_id
      );
      return;
    }

    let is_sdp_offer = signal.is_sdp_offer();
    let is_sdp_answer = signal.is_sdp_answer();

    // Recreate peer if it's a new offer and we had a peer before
    if let Some(peer_state) = self.peer_states.get(&user_id) {
      let got_first_offer = peer_state.got_first_offer();
      let is_initial_offerer = peer_state.is_initial_offerer();
      if is_sdp_offer && self.peers.contains_key(&user_id) && got_first_offer {
        info!("recreating peer.");
        let _ = self.remove_peer(&user_id, call_id.to_owned());
        // Re-create peer here.
        let _ = self
          .add_peer(user_id.to_owned(), is_initial_offerer, call_id)
          .await;
      }
    }

    if let Some(peer) = self.peers.get(&user_id) {
      let peer_ = peer.clone();

      async_runtime::spawn(async move { peer_.handle_signal(signal).await });
    } else {
      info!("adding to pending signals");
      self.pending_signals.add(&user_id, signal);
    }

    // Update first offer state
    if let Some(peer_state) = self.peer_states.get_mut(&user_id) {
      if is_sdp_offer {
        peer_state.set_got_first_offer();
      } else if is_sdp_answer {
        peer_state.set_got_first_answer();
      }
    }
  }

  fn toggle_mic(&mut self, enabled: bool) {
    if let Some(audio_input) = self.audio_input.as_ref() {
      audio_input.toggle(enabled)
    }
  }

  /// Emit an event to the app (eg. so it can send to the window)
  fn emit_event(&self, event: EngineEvent) {
    let listeners = self.events_listeners.lock().expect("poisened");

    for listener in listeners.iter() {
      info!("emit {:?}", event);
      (listener)(event.clone());
    }
  }

  fn is_current_call(&self, call_id: &CallId) -> bool {
    let is_call = self.call_id.as_ref().map_or(false, |c| c == call_id);
    is_call
  }
}
