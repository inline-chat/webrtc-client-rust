use bytes::Bytes;
use flume::{Receiver, Sender};
use serde_json::json;
use std::{
  sync::Arc,
  time::{Duration, Instant},
};
use str0m::{
  bwe::{Bitrate, BweKind},
  change::{DtlsCert, SdpOffer, SdpPendingOffer},
  channel::{ChannelData, ChannelId},
  error::NetError,
  format::Codec,
  media::{Direction, KeyframeRequestKind, MediaKind, MediaTime, Mid},
  net::{DatagramRecv, Receive, Transmit},
  IceConnectionState, Input, Output, Rtc,
};
// use tokio::net::UdpSocket;
use async_std::net::SocketAddr;

use crate::{
  debugger::{Debugger, StatKind},
  rtc::{
    commands::ConnectionState,
    peer::channel::{PeerChannelCommand, Sdp, Signal},
  },
};

use super::{
  engine::EngineSender,
  error::EngineError,
  ice::{
    self,
    gatherer::{Gathered, IceGatherer, IcePolicy},
  },
  peer::channel::{AudioMedia, PoolBytesVec},
  remote_control::RemoteControl,
  utils::UserId,
};
pub mod utils;

pub enum RunLoopEvent {
  LocalIceCandidate(Gathered),
  RemoteSignal(Signal),
  SendDataString(String),
  NetworkInput {
    source: SocketAddr,
    destination: SocketAddr,
    bytes: Bytes,
  },
  // old
  WriteAudio {
    data: Bytes,
    duration: MediaTime,
  },
  WriteMedia {
    kind: MediaKind,
    data: PoolBytesVec,
    rtp_time: MediaTime,
    extra: Option<MediaExtra>,
  },
  RequestScreenKeyframe,
  ConfigBwe(BweCfg),
  RestartIce,
  Destroy,
}

#[derive(Clone, Debug)]
pub enum BweCfg {
  Desired(Bitrate),
  Current(Bitrate),
  Reset(Bitrate),
}

impl BweCfg {
  pub fn desired(bitrate: Bitrate) -> Self {
    Self::Desired(bitrate)
  }

  pub fn current(bitrate: Bitrate) -> Self {
    Self::Current(bitrate)
  }

  pub fn reset(bitrate: Bitrate) -> Self {
    Self::Reset(bitrate)
  }
}

pub type PeerId = u16;

pub struct Peer2 {
  id: PeerId,
  run_loop_sender: Sender<RunLoopEvent>,
  pub initial_offerer: bool,

  task: tokio::task::JoinHandle<()>,
}

impl Peer2 {
  pub fn new(
    id: PeerId,
    user_id: UserId,
    initial_offerer: bool,
    engine_sender: EngineSender,
    debugger: Debugger,
    policy: IcePolicy,
    dtls_cert: DtlsCert,
    remote_control: RemoteControl,
  ) -> Self {
    // Comms with peer run loop
    let (run_loop_sender, run_loop_reveiver) = flume::bounded(1_000);

    let _debugger_ = debugger.clone();
    let engine_sender_ = engine_sender.clone();
    let user_id_ = user_id.clone();

    let mut run_loop = PeerRunLoop::new(
      id,
      &user_id,
      initial_offerer,
      run_loop_reveiver,
      run_loop_sender.clone(),
      engine_sender,
      debugger,
      policy,
      dtls_cert,
      remote_control,
    );

    let task = tokio::spawn(async move {
      // info!("duration = {}", duration.as_millis());
      if let Err(err) = run_loop.run().await {
        error!("Voice connection peer failed {}", err.to_string().as_str());
        let _ = engine_sender_.try_send(PeerChannelCommand::PeerStateChange {
          user_id: user_id_,
          peer_id: id,
          state: ConnectionState::Disconnected,
          ice_state: None,
        });
      }
    });

    Self {
      id,
      run_loop_sender,
      initial_offerer,
      task,
    }
  }

  pub fn id(&self) -> PeerId {
    self.id
  }

  pub async fn handle_signal(&self, signal: Signal) {
    let _ = self
      .run_loop_sender
      .send_async(RunLoopEvent::RemoteSignal(signal))
      .await;
  }

  pub async fn write_audio(&self, data: Bytes, duration: MediaTime) {
    let _ = self
      .run_loop_sender
      .try_send(RunLoopEvent::WriteAudio { data, duration });
  }

  pub fn request_screen_keyframe(&self) {
    let _ = self
      .run_loop_sender
      .try_send(RunLoopEvent::RequestScreenKeyframe);
  }

  pub fn restart_ice(&self) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::RestartIce);
  }

  pub fn send_data_string(&self, json_string: String) {
    let _ = self
      .run_loop_sender
      .try_send(RunLoopEvent::SendDataString(json_string));
  }

  pub fn config_bwe(&self, cfg: BweCfg) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::ConfigBwe(cfg));
  }

  pub async fn write_media(
    &self,
    kind: MediaKind,
    data: PoolBytesVec,

    rtp_time: MediaTime,
    extra: Option<MediaExtra>,
  ) {
    let _ = self.run_loop_sender.try_send(RunLoopEvent::WriteMedia {
      kind,
      data,

      rtp_time,
      extra,
    });
  }
}

impl Drop for Peer2 {
  fn drop(&mut self) {
    warn!("Dropping peer");
    let _ = self.run_loop_sender.try_send(RunLoopEvent::Destroy);
    self.task.abort();
  }
}

#[derive(Debug, Clone)]
pub enum MediaExtra {
  Audio { voice_activity: bool },
}

struct PeerRunLoop {
  id: PeerId,
  user_id: UserId,
  receiver: Receiver<RunLoopEvent>,
  engine_sender: EngineSender,
  rtc: Rtc,
  ice_gatherer: IceGatherer,
  ice_state: IceConnectionState,
  state: ConnectionState,
  mid: Option<Mid>,
  screen_mid: Option<Mid>,
  debugger: Debugger,
  pending_offer: Option<SdpPendingOffer>,
  last_rtp_time: MediaTime,
  initial_offerer: bool,
  channel_id: Option<ChannelId>,
  remote_control: RemoteControl,
  signaling_started: bool,
}

impl Drop for PeerRunLoop {
  fn drop(&mut self) {
    self.rtc.disconnect();
  }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaType {
  Voice,
  Screen,
}

const AUDIO_PT: u8 = 111;

impl PeerRunLoop {
  pub fn new(
    id: PeerId,
    user_id: &UserId,
    initial_offerer: bool,
    run_loop_receiver: Receiver<RunLoopEvent>,
    run_loop_sender: Sender<RunLoopEvent>,
    engine_sender: EngineSender,
    debugger: Debugger,
    policy: IcePolicy,
    dtls_cert: DtlsCert,
    remote_control: RemoteControl,
  ) -> Self {
    // create rtc
    let mut rtc = Rtc::builder()
      .clear_codecs()
      .enable_opus(true)
      .set_ice_lite(false)
      .set_send_buffer_audio(50)
      .set_reordering_size_audio(4)
      .set_reordering_size_video(30)
      .set_dtls_cert(dtls_cert)
      .enable_bwe(Some(Bitrate::kbps(40)))
      .set_stats_interval(Duration::from_secs(10).into());

    let codec_config = rtc.codec_config();

    // h264
    const PARAMS: &[(u8, u8, bool, u32)] =
      &[(114, 115, true, 0x64001f), (123, 119, true, 0x4d001f)];

    for p in PARAMS {
      codec_config.add_h264(p.0.into(), Some(p.1.into()), p.2, p.3)
    }

    let mut rtc = rtc.build();

    rtc.bwe().set_current_bitrate(Bitrate::kbps(40));
    rtc.bwe().set_desired_bitrate(Bitrate::kbps(40));

    let ice_gatherer = ice::gatherer::IceGatherer::new(run_loop_sender.clone(), policy);
    let signaling_started = false;

    Self {
      id,
      user_id: user_id.to_owned(),
      engine_sender,
      receiver: run_loop_receiver,
      rtc,
      state: ConnectionState::Connecting,
      ice_state: IceConnectionState::New,
      ice_gatherer,
      pending_offer: None,
      last_rtp_time: MediaTime::ZERO,
      initial_offerer,
      debugger,
      mid: None,
      screen_mid: None,
      channel_id: None,
      remote_control,
      signaling_started,
    }
  }

  async fn init(&mut self) -> Result<(), EngineError> {
    // start gathering
    self.ice_gatherer.gather();

    if self.initial_offerer {
      // only initial offerer do it
      info!("initial offerer");
      let mut changes = self.rtc.sdp_api();

      let audio_mid = changes.add_media(MediaKind::Audio, Direction::SendRecv, None, None);
      let screen_mid = changes.add_media(
        MediaKind::Video,
        Direction::SendRecv,
        None,
        Some("s".into()),
      );
      let channel_id = changes.add_channel("main".to_string());

      self.mid = audio_mid.into();
      self.screen_mid = screen_mid.into();
      self.channel_id = channel_id.into();

      if let Some((offer, pending_offer)) = changes.apply() {
        // save for later
        self.pending_offer = pending_offer.into();
        self.send_offer(offer);
      } else {
        unreachable!("did not get initial offer");
      }
    }

    Ok(())
  }

  fn send_offer(&mut self, offer: SdpOffer) {
    let _sdp = offer.to_sdp_string();
    debug!("sending offer");
    // dbg!(sdp);
    self.send_to_engine(PeerChannelCommand::SendSignal {
      user_id: self.user_id.to_owned(),
      signal: Signal::Sdp(Sdp::Offer(offer)),
    });
  }

  async fn run(&mut self) -> Result<(), EngineError> {
    loop {
      if !self.rtc.is_alive() {
        break;
      }

      if !self.signaling_started {
        info!("started signaling");
        self.signaling_started = true;
        self.init().await?;
      }

      // info!("poll_output");
      let operation = if let Ok(output) = self.rtc.poll_output() {
        self.handle_output(output).await
      } else {
        None
      };

      let duration = if let Some(timeout) = operation {
        (timeout - Instant::now()).max(Duration::from_nanos(1))
      } else {
        continue;
      };

      // info!("duration = {}", duration.as_millis());
      tokio::select! {
        Ok(event) = self.receiver.recv_async() => {
          match event {
            RunLoopEvent::Destroy => {
              break;
            }
            _ => {}
          }
          self.handle_command(event).await?;
          continue;
        }

        Some(gathered) = self.ice_gatherer.poll_candidate() => {
          self.handle_command(RunLoopEvent::LocalIceCandidate(gathered)).await?;
          continue;
        }

        _ = tokio::time::sleep(duration) => {
        }
      }

      // Drive time forward in all clients.
      let now = Instant::now();
      let _ = self.rtc.handle_input(Input::Timeout(now));
    }

    info!("peer run loop ended");
    Ok(())
  }

  async fn handle_output(&mut self, output: Output) -> Option<Instant> {
    match output {
      Output::Event(event) => {
        match event {
          str0m::Event::Connected => {
            debug!("--- connected ---");
            self.state = ConnectionState::Connected;

            self.send_to_engine(PeerChannelCommand::PeerStateChange {
              user_id: self.user_id.clone(),
              peer_id: self.id,
              state: ConnectionState::Connected,
              ice_state: None,
            });
          }
          str0m::Event::KeyframeRequest(_) => {
            // TODO: Request keyframe from screen-share encoder here
            let _ = self
              .engine_sender
              .try_send(PeerChannelCommand::ScreenKeyFrameRequest);
          }
          str0m::Event::MediaChanged(_change) => {
            info!("str0m::Event::MediaChanged");
          }
          str0m::Event::MediaAdded(media_added) => {
            info!("str0m::Event::MediaAdded");
            info!("media added ");
            info!("media_added mid {:?}", media_added.mid);
            info!("media_added {:?}", media_added.direction);

            match media_added.kind {
              MediaKind::Audio => {
                self.mid = Some(media_added.mid);
                let writer = self
                  .rtc
                  .writer(media_added.mid)
                  .expect("unable to get writer");
                let mut clock_rate = 48_000;
                let mut channels = 2_u16;

                for param in writer.payload_params() {
                  let spec = param.spec();
                  clock_rate = spec.clock_rate.get();
                  channels = spec.channels.unwrap_or(2) as u16;
                }

                self.send_to_engine(PeerChannelCommand::MediaAdded {
                  user_id: self.user_id.clone(),
                  media: AudioMedia {
                    clock_rate,
                    channels,
                  },
                });
              }

              MediaKind::Video => {
                info!("got screen_mid {:#?}", media_added.mid);

                let writer = self
                  .rtc
                  .writer(media_added.mid)
                  .expect("unable to get writer");

                for param in writer.payload_params() {
                  let spec = param.spec();

                  info!("got PT for video {:#?}", param.pt());
                  info!("got spec for video {:#?}", param.spec());

                  // if spec.codec == Codec::H264 && spec.format.profile_level_id == Some(0x4d001f) {
                  // if spec.codec == Codec::H264 && spec.format.profile_level_id == Some(0x64001F)
                  // && spec.format.profile_level_id == Some(0x64001f)
                  if spec.codec == Codec::H264 {
                    info!("Picked screen mid");
                    self.screen_mid = Some(media_added.mid);
                  }
                }
              }
            }
          }
          str0m::Event::MediaData(data) => {
            // data.data

            if self.mid.is_none() {
              return None;
            }

            let media_type = if data.mid == self.mid.expect("expected mid") {
              MediaType::Voice
            } else if data.mid == self.screen_mid.expect("expected screen") {
              MediaType::Screen
            } else {
              unreachable!("unknown media type");
            };

            // if media_type == MediaType::Screen {
            //   info!("got media MediaData {:#?} ", media_type);
            // }

            self.send_to_engine(PeerChannelCommand::IncomingMediaData {
              user_id: self.user_id.clone(),
              data: Arc::new(data),
              media_type,
            });

            // todo: handle incoming media data
          }
          str0m::Event::IceConnectionStateChange(ice_state) => {
            self.ice_state = ice_state;
            info!("IceConnectionStateChange = {:#?}", &ice_state);
            let state: Option<ConnectionState> = match ice_state {
              IceConnectionState::Checking | IceConnectionState::Disconnected => {
                ConnectionState::Connecting.into()
              }
              IceConnectionState::Connected => ConnectionState::Connected.into(),
              _ => None,
            };

            if let Some(connection_state) = state {
              if connection_state == ConnectionState::Connected
                && self.state != ConnectionState::Connected
              {
                // don't do anything
              } else {
                self.send_to_engine(PeerChannelCommand::PeerStateChange {
                  user_id: self.user_id.clone(),
                  peer_id: self.id,
                  state: connection_state,
                  ice_state: Some(ice_state),
                });
              }
            }

            let state_string = match ice_state {
              str0m::IceConnectionState::New => "New",
              str0m::IceConnectionState::Checking => "Checking",
              str0m::IceConnectionState::Disconnected => "Disconnected",
              str0m::IceConnectionState::Completed => "Completed",
              str0m::IceConnectionState::Connected => "Connected",
            };
            self.debugger.stat(
              StatKind::IceConnectionStatus,
              json!(state_string),
              Some(&self.user_id),
            );
          }
          str0m::Event::ChannelOpen(id, _) => {
            self.channel_id = id.into();
            info!("data channel opened {:#?}", id);
          }

          str0m::Event::ChannelData(data) => {
            self.handle_channel_data(data);
          }

          str0m::Event::MediaEgressStats(stats) => {
            self
              .debugger
              .stat(StatKind::EgressRtt, json!(stats.rtt), Some(&self.user_id));
            // self
            //   .debugger
            //   .stat(StatKind::EgressLoss, json!(stats.loss), Some(&self.user_id))
          }

          str0m::Event::MediaIngressStats(stats) => {
            self
              .debugger
              .stat(StatKind::IngressRtt, json!(stats.rtt), Some(&self.user_id));
          }

          str0m::Event::EgressBitrateEstimate(kind) => match &kind {
            BweKind::Twcc(bitrate) => {
              self.send_to_engine(PeerChannelCommand::SetScreenBitrate(bitrate.clone()));
              // self.rtc.bwe().set_current_bitrate(bitrate.clone());

              self.debugger.stat(
                StatKind::EgressBandwidthEstimate,
                json!(bitrate.to_string()),
                Some(&self.user_id),
              );
            }
            _ => {}
          },

          _ => {
            // dbg!(event);
          }
        }

        None
      }
      Output::Timeout(timeout) => Some(timeout),
      Output::Transmit(transmit) => {
        self.transmit(transmit).await;
        None
      }
    }
  }

  async fn handle_command(&mut self, event: RunLoopEvent) -> Result<(), EngineError> {
    match event {
      RunLoopEvent::ConfigBwe(BweCfg::Reset(bitrate)) => {
        println!("RunLoopEvent::ConfigBwe::Reset {:#?}", &bitrate);
        self.rtc.bwe().reset(bitrate);
        Ok(())
      }
      RunLoopEvent::ConfigBwe(BweCfg::Desired(bitrate)) => {
        println!("RunLoopEvent::ConfigBwe::Desired {:#?}", &bitrate);
        self.rtc.bwe().set_desired_bitrate(bitrate);
        Ok(())
      }
      RunLoopEvent::ConfigBwe(BweCfg::Current(bitrate)) => {
        println!("RunLoopEvent::ConfigBwe::Current {:#?}", &bitrate);
        self.rtc.bwe().set_current_bitrate(bitrate);
        Ok(())
      }
      RunLoopEvent::ConfigBwe(BweCfg::Desired(bitrate)) => {
        println!("RunLoopEvent::ConfigBwe::Desired {:#?}", &bitrate);
        self.rtc.bwe().set_desired_bitrate(bitrate);
        Ok(())
      }
      RunLoopEvent::NetworkInput {
        bytes,
        destination,
        source,
      } => {
        if !bytes.is_empty() {
          let s: Result<DatagramRecv, NetError> = bytes.as_ref().try_into();
          if let Ok(datagram) = s {
            // to prevent panic in net/src/lib.rs:137:21
            self.rtc.handle_input(Input::Receive(
              Instant::now(),
              // create input
              Receive {
                source,
                destination,
                contents: datagram,
                proto: str0m::net::Protocol::Udp,
              },
            ))?;
          } else {
            warn!("failed to convert bytes to datagram");
          }
        } else {
          // todo handle error
          warn!("bytes len is 0");
        }
        Ok(())
      }

      RunLoopEvent::WriteAudio { data, duration } => {
        if !data.is_empty() {
          // to prevent panic in net/src/lib.rs:137:21
          self.send_audio_data(data.as_ref(), duration);
        } else {
          warn!("media bytes len is 0");
        }
        // info!("write media");
        Ok(())
      }

      RunLoopEvent::WriteMedia {
        kind,
        data,
        rtp_time,
        extra,
      } => {
        self.write_media(kind, &data[..], rtp_time, extra);

        Ok(())
      }

      RunLoopEvent::SendDataString(json_string) => {
        self.send_data_string(json_string);
        Ok(())
      }

      RunLoopEvent::RequestScreenKeyframe => {
        self.screen_request_keyframe();
        Ok(())
      }

      RunLoopEvent::RestartIce => {
        self.ice_gatherer.gather();
        self.rtc.sdp_api().ice_restart(true); // false?
        Ok(())
      }

      RunLoopEvent::RemoteSignal(signal) => self.handle_signal(signal),

      RunLoopEvent::LocalIceCandidate(gathered) => {
        self.rtc.add_local_candidate(gathered.local);

        // send to the other side
        self.send_to_engine(PeerChannelCommand::SendSignal {
          user_id: self.user_id.to_owned(),
          signal: Signal::Candidate(gathered.remote.clone()),
        });

        self.debugger.log(
          crate::debugger::LogKind::LocalIceCandidate,
          json!(gathered.remote.to_string()),
          Some(&self.user_id),
        );
        Ok(())
      }

      _ => Ok(()),
    }
  }

  fn handle_signal(&mut self, signal: Signal) -> Result<(), EngineError> {
    match signal {
      Signal::Candidate(candidate) => {
        // ICE Candidate
        self.rtc.add_remote_candidate(candidate.clone());

        self.debugger.log(
          crate::debugger::LogKind::RemoteIceCandidate,
          json!(candidate.to_string()),
          Some(&self.user_id),
        );
        Ok(())
      }
      Signal::Sdp(Sdp::Offer(offer)) => {
        // dbg!(&offer);
        info!("got offer =====");

        // TODO: Ignore signals conflicting with our state and send back a request to reconnect

        // accept offer
        let answer = self.rtc.sdp_api().accept_offer(offer)?;

        // send answer
        self.send_to_engine(PeerChannelCommand::SendSignal {
          user_id: self.user_id.to_owned(),
          signal: Signal::Sdp(Sdp::Answer(answer)),
        });
        Ok(())
      }

      Signal::Sdp(Sdp::Answer(answer)) => {
        if let Some(changes) = self.pending_offer.take() {
          // dbg!(&answer);

          self.rtc.sdp_api().accept_answer(changes, answer)?;

          self.send_to_engine(PeerChannelCommand::MediaAdded {
            user_id: self.user_id.clone(),
            media: AudioMedia {
              clock_rate: 48_000,
              channels: 2,
            },
          });
        } else {
          warn!("no pending changes for answer");
        }
        Ok(())
      }
    }
  }

  fn send_to_engine(&mut self, command: PeerChannelCommand) {
    let _ = self.engine_sender.try_send(command);
  }

  async fn transmit(&self, transmit: Transmit) {
    self.ice_gatherer.send_to(transmit).await;
  }

  fn send_audio_data(&mut self, data: &[u8], duration: MediaTime) {
    // skip sending
    if self.state != ConnectionState::Connected {
      return;
    }

    let Some(mid) = self.mid else {
      return;
    };

    let Some(writer) = self.rtc.writer(mid) else {
      return;
    };

    // 111
    let Some(pt) = writer
      .payload_params()
      .find(|p| {
        let spec = p.spec();
        spec.codec == Codec::Opus && spec.clock_rate.get() == 48_000
      })
      .map(|p| p.pt())
    else {
      return;
    };

    let now = Instant::now();
    let rtp_time = self.last_rtp_time;
    if let Err(e) = writer.write(
      pt, // todo: get this from encoder
      now, rtp_time, data,
    ) {
      warn!("Client ({}) failed: {:?}", &self.user_id, e);
      self.rtc.disconnect();
    }

    self.last_rtp_time = rtp_time + duration;
  }

  fn write_media(
    &mut self,
    kind: MediaKind,
    data: &[u8],
    rtp_time: MediaTime,
    extra: Option<MediaExtra>,
  ) {
    // to prevent panic in net/src/lib.rs:137:21
    if data.is_empty() {
      return;
    }

    // skip sending
    if self.state != ConnectionState::Connected {
      return;
    }

    let now = Instant::now();

    let result = match kind {
      MediaKind::Audio => {
        let Some(mid) = self.mid else {
          return;
        };

        let Some(writer) = self.rtc.writer(mid) else {
          return;
        };

        // 111
        // let Some(pt) = writer
        //   .payload_params()
        //   .iter()
        //   .find(|p| {
        //     let spec = p.spec();
        //     spec.codec == Codec::Opus && spec.clock_rate == 48_000
        //   })
        //   .map(|p| p.pt())
        // else {
        //   return;
        // };

        writer
          .audio_level(
            -30, // todo
            match extra {
              Some(MediaExtra::Audio { voice_activity }) => voice_activity,
              _ => true,
            },
          )
          .write(
            AUDIO_PT.into(), // todo: get this from encoder
            now,
            rtp_time,
            data,
          )
      }

      MediaKind::Video => {
        let Some(mid) = self.screen_mid else {
          return;
        };

        trace!("has mid {:#?}", mid);

        let Some(writer) = self.rtc.writer(mid) else {
          return;
        };

        let Some(pt) = writer
          .payload_params()
          .find(|p| {
            let spec = p.spec();
            spec.codec == Codec::H264 // && spec.format.profile_level_id == Some(0x64001f)
                                      // spec.codec == Codec::H264  && spec.format.profile_level_id == Some(0x64001f)
                                      // || spec.codec == Codec::H264 && spec.format.profile_level_id == Some(0x4d001f)
                                      // || spec.codec == Codec::H264 && spec.format.profile_level_id == Some(0x640034)
                                      // || spec.codec == Codec::H264 && spec.format.profile_level_id == Some(0x64001F)
          })
          .map(|p| p.pt())
        else {
          error!("pt not found for screen");
          return;
        };

        trace!("sending video {:#?} {:#?} {:?}", mid, pt, data.len());

        writer.write(pt, now, rtp_time, data)
      }
    };

    if let Err(e) = result {
      warn!("Client ({}) failed: {:?}", &self.user_id, e);
      self.rtc.disconnect();
    }
  }

  fn screen_request_keyframe(&mut self) {
    if let Some(mid) = self.screen_mid {
      if let Some(mut writer) = self.rtc.writer(mid) {
        // if writer.is_request_keyframe_possible(KeyframeRequestKind::Pli) {
        info!("Requesting keyframe for screen");
        let _ = writer.request_keyframe(None, KeyframeRequestKind::Fir);
        // }
      }
    }
  }
  fn handle_channel_data(&mut self, data: ChannelData) {
    if data.binary {
      return;
    }

    // how to stringify:
    // dbg!(serde_json::to_string(&DataChannelEvent::MouseDown { x: 100, y: 100 }).unwrap());

    let data = serde_json::from_slice::<DataChannelEvent>(data.data.as_slice());
    if let Ok(event) = data {
      match event {
        DataChannelEvent::Signal { signal: _ } => {
          todo!();
          // let signal = sdp::parse_signal_json(signal);
          // let _ = self.handle_signal(signal);
        }
        _ => {
          self.remote_control.event(self.user_id.clone(), event);
        }
      }
    } else {
      warn!("failed to parse data channel event");
    }
  }

  pub fn send_data_string(&mut self, data_string: String) {
    let Some(channel_id) = self.channel_id else {
      warn!("failed to find channel id");
      return;
    };
    let Some(mut channel) = self.rtc.channel(channel_id) else {
      warn!("failed to get channel but have channel id");
      return;
    };

    //convert this  to event
    let buf = data_string.as_bytes();
    let _ = channel.write(false, buf);
  }
}

use serde::{self, Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum DataChannelEvent {
  MouseMove {
    x: f64,
    y: f64,
  },
  MouseDown {
    x: f64,
    y: f64,
    // 0 is left, 1 is middle, 2 is right
    button: usize,
    // modifiers
    // ctrl: bool,
    // alt: bool,
    // shift: bool,
    // meta: bool,
  },
  Signal {
    // signal.to_json()
    signal: String,
  },
}
