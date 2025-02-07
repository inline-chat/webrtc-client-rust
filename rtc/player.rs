use std::{
  collections::HashMap,
  sync::{atomic::AtomicU64, Arc},
  time::{Duration, Instant},
};

use audio_thread_priority::{
  demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};
use cap::cv::current_host_time;
use cpal::{
  traits::{DeviceTrait, StreamTrait},
  BufferSize, StreamConfig,
};
use flume::{Receiver, Sender};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};
use str0m::media::MediaData;

use crate::{
  debugger::{Debugger, StatKind},
  rtc::audio::{cpal_err_fn, get_default_output_device, get_output_device},
};

use super::{
  audio_decoder::TrackDecoder,
  jitter::MAX_10MS_SAMPLE_COUNT,
  peer::channel::AudioMedia,
  processor::AudioEchoProcessor,
  resampler::{self, Resampler, ResamplerConfig},
  utils::UserId,
};

pub struct PlayerOptions {
  pub echo_processor: AudioEchoProcessor,
  pub preferred_device: Option<String>,
  pub debugger: Debugger,
}

/// Mix, resample and play audio (one instance)
pub struct Player {
  echo_processor: AudioEchoProcessor,
  command_receiver: Receiver<PlayerCommand>,
  debugger: Debugger,
  // device_close_sender: Sender<bool>,
  controller: PlayerController,
  producer: HeapProducer<f32>,
  #[allow(unused)]
  resampler: Resampler,
  mix_buffer: Vec<f32>,
  tracks: HashMap<UserId, TrackDecoder>,
  #[allow(unused)]
  volume: f32,
  config: OutputConfig,
  consumer: Option<HeapConsumer<f32>>,
  chunks_buffer: Vec<Vec<f32>>,
  channels: i32,
  frame_size: usize,
  device_latency: Arc<AtomicU64>,
}

pub enum PlayerCommand {
  AddMedia(UserId, AudioMedia),
  RemoveMedia(UserId),
  MediaData(UserId, Arc<MediaData>),
  // ChangeDevice(String),
  // SetVolume(f32),
  Stop,
}

#[derive(Clone)]
pub struct PlayerController {
  player_sender: Sender<PlayerCommand>,
}
impl PlayerController {
  pub fn new(player_sender: Sender<PlayerCommand>) -> Self {
    Self { player_sender }
  }

  pub fn add_media(&self, user_id: UserId, media: AudioMedia) {
    let _ = self
      .player_sender
      .try_send(PlayerCommand::AddMedia(user_id, media));
  }

  pub fn add_media_data(&self, user_id: UserId, media_data: Arc<MediaData>) {
    let _ = self
      .player_sender
      .try_send(PlayerCommand::MediaData(user_id, media_data));
  }

  pub fn remove_media(&self, user_id: UserId) {
    let _ = self
      .player_sender
      .try_send(PlayerCommand::RemoveMedia(user_id));
  }

  #[allow(unused)]
  pub fn change_device(&self) {
    todo!();
  }

  #[allow(unused)]
  pub fn set_volume(&self) {
    todo!();
  }

  pub fn stop(&self) {
    let _ = self.player_sender.try_send(PlayerCommand::Stop);
  }
}

impl Drop for PlayerController {
  fn drop(&mut self) {
    self.stop();
    info!("Player controller dropped");
  }
}

impl Player {
  pub fn new(
    PlayerOptions {
      echo_processor,
      debugger,
      ..
    }: PlayerOptions,
  ) -> Self {
    let (command_sender, command_receiver) = flume::bounded::<PlayerCommand>(10);
    let config = Self::get_output_device();
    debug!("Player config: {:?}", &config.3);
    let (producer, consumer) = Self::create_ringbuffer(config.3).split();

    // TODO: we can have another signal to change device in the middle of playing

    let device_sample_rate = config.1;
    let device_channels = config.2;
    let resampler = Resampler::new(ResamplerConfig {
      input_sample_rate: 48_000, // Hard coded from track decoders
      output_sample_rate: device_sample_rate,
      channels: device_channels,
      chunk: resampler::Chunk::TenMs,
    });

    // hard coded from track decoder
    let channels = 2;
    let frame_size = echo_processor.num_samples_per_frame() * channels as usize;

    Self {
      controller: PlayerController::new(command_sender),
      echo_processor,
      debugger,
      command_receiver,
      consumer: Some(consumer),
      producer,
      resampler,
      mix_buffer: vec![0.0; MAX_10MS_SAMPLE_COUNT],
      tracks: HashMap::new(),
      volume: 1.0,
      channels,
      frame_size,
      config,
      chunks_buffer: vec![vec![0.0; MAX_10MS_SAMPLE_COUNT]; 20],
      device_latency: Arc::new(AtomicU64::new(0)),
    }
  }

  pub fn get_controller(&self) -> PlayerController {
    self.controller.clone()
  }

  fn real_buffer_size(&self, buffer_size: u32) -> u32 {
    (buffer_size.max(512) as f32 * 48.0 / 44.1 * 2_f32).ceil() as u32
  }

  pub async fn run(&mut self) -> Result<(), anyhow::Error> {
    let (close_sender, close_receiver) = flume::bounded::<()>(1);

    let _buffer_size = self.real_buffer_size(self.config.3.to_owned());

    // ... on a thread that will compute audio and has to be real-time: buffer_size 0 will auto select from sample rate
    let prio_handle = match promote_current_thread_to_real_time(0, 48_000) {
      Ok(h) => Some(h),
      Err(e) => {
        eprintln!("Error promoting player thread to real-time: {}", e);
        None
      }
    };

    let processor = self.echo_processor.clone();
    // keep here to have stream play
    let (_, __, ___, stream) = Self::start_output_device(
      self.consumer.take().expect("to have consumer"),
      &self.config,
      processor,
      self.device_latency.clone(),
    );

    // NEW
    let mut interval = tokio::time::interval(Duration::from_millis(10)); // we operate in 10ms chunks
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip); // Skip because it's only skipped if more than 5ms

    loop {
      tokio::select! {
        // handle player commands
        Ok(command) = self.command_receiver.recv_async() => {
          self.handle_command(command, close_sender.clone()).await;
        }

        // every 10ms play audio
        _ = interval.tick() => {
          self.audio_tick().await;
        }

        // when player should end its operations
        _ = close_receiver.recv_async() => {
          break;
        }
      }
    }

    drop(stream);

    if let Some(handle) = prio_handle {
      let _ = demote_current_thread_from_real_time(handle);
    }

    info!("player loop ended.");
    Ok(())
  }

  async fn handle_command(&mut self, command: PlayerCommand, close_sender: Sender<()>) {
    match command {
      PlayerCommand::AddMedia(user_id, media) => {
        let track_decoder =
          TrackDecoder::new(user_id.to_owned(), self.debugger.clone(), media).await;
        // add to the list of remote tracks so we poll it in the run loop
        self.tracks.insert(user_id, track_decoder);
      }
      PlayerCommand::MediaData(user_id, data) => {
        if let Some(track) = self.tracks.get_mut(&user_id) {
          let v = data.ext_vals.voice_activity.clone().unwrap_or(false);
          //These can add delay to audio look
          // self.debugger.stat(
          //   StatKind::VoiceActivity,
          //   if v { "true".into() } else { "false".into() },
          //   Some(&user_id),
          // );

          track.insert_packet(data);
        } else {
          debug!("no track to add packet");
        }
      }
      PlayerCommand::RemoveMedia(user_id) => {
        self.tracks.remove(&user_id);
        debug!("PlayerCommand::RemoveMedia")
      }
      PlayerCommand::Stop => {
        let _ = close_sender.try_send(());
      }
    }
  }

  async fn audio_tick(&mut self) {
    // get track decoder audios
    // get audio from all decoder tasks

    // Remove this allocation
    if self.chunks_buffer.len() < self.tracks.len() {
      panic!("does not support +20");
    }

    if self.tracks.is_empty() {
      return;
    }

    if self.producer.is_full() {
      return;
    }

    let active_tracks_len = self.tracks.len();
    let _processing_start_instant = Instant::now();

    // fill out the buffer
    for (index, (_, track)) in self.tracks.iter_mut().enumerate() {
      let buffer = self
        .chunks_buffer
        .get_mut(index)
        .expect("to have chunks buffer");
      // buffer.clear();
      let size = track.get_audio(buffer);
      // probably fix audio quality
      buffer.truncate(size);
    }

    ////// MIX
    self.mix_buffer.clear();
    let chunk_size = self.chunks_buffer[0].len(); // each audio track chunk size

    // do not loop over old chunks left over
    let active_chunks_buffers = &self.chunks_buffer[0..active_tracks_len];

    // safety check
    for chunk in active_chunks_buffers.iter() {
      assert!(
        chunk.len() == chunk_size,
        "player track output size is not equal to other chunks"
      );
    }

    for i in 0..chunk_size {
      // go over each sample and add them together
      let mut sample: f32 = 0.0;
      for chunk in active_chunks_buffers.iter() {
        sample += chunk[i];
      }
      // Note: Removed min max clipping to avoid altering stream for the echo processor
      // todo: maybe batch a slice to lower volume?
      sample = sample.max(-1.0).min(1.0);
      self.mix_buffer.push(sample);
    }

    assert!(
      self.mix_buffer.len() == chunk_size,
      "mix buffer size is not equal to chunk size before mix"
    );

    //// PROCESS

    // Now process the in 480 sample frames and push to ringbuffer
    let samples = &mut self.mix_buffer[..];
    let _full_frame_len = samples.len();
    let _sample_rate = 48_000;
    let _channels = 2;
    let decoder_output_exact = samples.chunks_exact_mut(self.frame_size);

    // First, let's calculate stream delay by estimating when these frames
    // that we're processing will be played by the audio hardware
    // we use 960 here because we process 960 samples (2 frames) every 10ms
    // 960 is equivalent to 10ms of 48khz stereo audio

    // let buffer_delay_us =
    //   ((1e6 * buffer_samples as f64 / sample_rate as f64 / channels as f64) + 0.5) as u64;
    // let processing_delay_us = processing_start_instant.elapsed().as_micros() as u64;
    // let device_latency_us = self.device_latency.load(Ordering::Relaxed);
    // // let device_latency_us = 0; // don't use this unreliable value for now.
    // let render_delay_us = processing_delay_us + device_latency_us + buffer_delay_us;
    // dbg!(buffer_delay_us);
    // dbg!(processing_delay_us);
    // dbg!(device_latency_us);
    // self.echo_processor.set_playback_delay_us(render_delay_us);

    for frame in decoder_output_exact {
      // check
      if self.producer.free_len() < frame.len() {
        error!("producer buffer is full, ignoring");
        continue;
      }

      trace!("processing render frame");
      // process
      self.echo_processor.process_render_frame(frame);

      // play
      self.producer.push_slice(frame);

      // log
      // let debugger = self.debugger.clone();
      // let len = self.producer.len();
      // tokio::spawn(async move {
      //   debugger.stat(StatKind::PlayerBufferSamples, len.into(), None);
      //   debugger.stat(StatKind::PlayerDecodedFrameLen, full_frame_len.into(), None);
      // });
    }
  }

  fn get_output_device() -> OutputConfig {
    // Config
    let output_config = get_output_device();
    info!("output config {:?}", output_config);

    let channels = output_config.channels;
    assert!(channels == 2, "output device must be stereo");
    let sample_rate = output_config.sample_rate.0;
    let buffer_size = if let BufferSize::Fixed(h) = output_config.buffer_size {
      h
    } else {
      warn!("failed to get fixed buffer size");
      512 // todo: make better
    };

    OutputConfig(output_config, sample_rate, channels, buffer_size)
  }

  fn start_output_device(
    mut consumer: HeapConsumer<f32>,
    config: &OutputConfig,
    _processor: AudioEchoProcessor,
    _device_latency: Arc<AtomicU64>,
  ) -> (u16, u32, Sender<bool>, cpal::Stream) {
    let OutputConfig(output_config, sample_rate, channels, _) = config;
    let (close_sender, _) = flume::bounded::<bool>(1);
    let sample_rate_ = *sample_rate;
    let channels_ = *channels;

    // consumer
    let output_data_fn = move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
      let count = consumer.pop_slice(data);

      trace!("output device data {}", data.len());

      // fill the rest with 0.0
      for i in count..data.len() {
        data[i] = 0.0;
      }

      // Delay calc after play
      let render_time_ns = info.timestamp().callback.as_nanos();
      let now_time_ns = host_time_to_stream_instant(current_host_time()).as_nanos();
      let device_latency_us = 1e-3 * (render_time_ns - now_time_ns) as f64;
      let buffer_samples = consumer.len();
      let buffer_delay_us =
        ((1e6 * buffer_samples as f64 / sample_rate_ as f64 / channels_ as f64) + 0.5) as f64;
      let render_latency_ms = (1e-3 * (device_latency_us + buffer_delay_us) + 0.5) as u64;

      // maybe move this to a tokio task.
      _processor.set_playback_delay_ms(render_latency_ms);
    };

    debug!("output config={:#?}", &output_config);

    // output_config_clone
    let output_stream = get_default_output_device()
      .expect("Could not get default output default device")
      .build_output_stream(output_config, output_data_fn, cpal_err_fn)
      .expect("failed to create output stream");

    output_stream
      .play()
      .expect("failed to play in output device");

    (*channels, *sample_rate, close_sender, output_stream)
  }

  fn create_ringbuffer(buffer_size: u32) -> HeapRb<f32> {
    // let max_output_device_readsize = buffer_size as usize * 3;
    // buffer_size can be zero
    let _actual_buffer_size = buffer_size as f32 * 48.0 / 44.1 * 2_f32;
    // let max_output_device_readsize = (buffer_size as usize * 3).max(480);
    let _max_output_device_readsize = 960_f32 * 3_f32; // 2 times of decoded frame + 1 time of output buffer size
                                                       // let max_output_device_readsize = 960_f32 + actual_buffer_size * 4_f32; // 2 times of decoded frame + 1 time of output buffer size
    let max_output_device_readsize = 960_f32 * 4_f32; // increased because I increased buffer size to 480

    #[cfg(target_os = "linux")]
    let max_output_device_readsize = buffer_size as f32 * 18_f32; // 240 * 18
    info!("max_output_device_readsize {}", &max_output_device_readsize);

    HeapRb::new(max_output_device_readsize.ceil() as usize)
  }
}

impl Drop for Player {
  fn drop(&mut self) {
    info!("player dropped");
  }
}

struct OutputConfig(StreamConfig, u32, u16, u32);

#[cfg(test)]
mod tests {

  #[test]
  fn trying_to_write_test() {
    assert!(true);
  }
}

pub fn host_time_to_stream_instant(m_host_time: u64) -> cpal::StreamInstant {
  let info = cap::cidre::mach::TimeBaseInfo::new();
  let nanos = m_host_time * info.numer as u64 / info.denom as u64;
  let secs = nanos / 1_000_000_000;
  let subsec_nanos = nanos - secs * 1_000_000_000;

  cpal::StreamInstant::new(secs as i64, subsec_nanos as u32)
}
