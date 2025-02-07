use super::{
  audio::AudioError,
  echo_cancel::EncoderProcess,
  net::bytes_pool::{self, BytesPool},
  peer::channel::{PeerChannelCommand, PoolBytesVec},
  processor::AudioEchoProcessor,
  resampler::{Resampler, ResamplerConfig},
};
use crate::rtc::{
  audio::{cpal_err_fn, get_input_device, get_opus_samples_count},
  peer2::MediaExtra,
  player::host_time_to_stream_instant,
  resampler,
};
use audio_thread_priority::{
  demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};
use bytes::BytesMut;
use cpal::traits::{DeviceTrait, StreamTrait};
use flume::{Receiver, Sender};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb, Rb};
use std::{
  sync::atomic::{AtomicU64, Ordering},
  sync::Arc,
  time::Duration,
};
use str0m::media::{Frequency, MediaKind, MediaTime};

pub enum AudioInputCommand {
  Toggle(bool),

  ChangeDevice(String),
  // internal
  GotNewAudio,
  Stop,
}

pub struct AudioInputOptions {
  pub preferred_mic: Option<String>,
  pub enabled: bool,
  pub echo_processor: AudioEchoProcessor,
  pub engine_sender: Sender<PeerChannelCommand>,
}

pub struct AudioInput {
  commands_sender: Sender<AudioInputCommand>,
  event_loop: Option<AudioInputEventLoop>,
}

impl AudioInput {
  pub fn new(options: AudioInputOptions) -> Self {
    let (commands_sender, commands_reciever) = flume::bounded::<AudioInputCommand>(500);

    let event_loop = AudioInputEventLoop::new(options, commands_sender.clone(), commands_reciever);

    Self {
      commands_sender,
      event_loop: Some(event_loop),
    }
  }

  pub fn get_event_loop(&mut self) -> AudioInputEventLoop {
    self.event_loop.take().expect("to have input event loop")
  }

  pub fn toggle(&self, enabled: bool) {
    let _ = self
      .commands_sender
      .try_send(AudioInputCommand::Toggle(enabled));
  }

  pub fn change_device(&self, device: String) {
    let _ = self
      .commands_sender
      .try_send(AudioInputCommand::ChangeDevice(device));
  }

  pub fn stop(&self) {
    let _ = self.commands_sender.try_send(AudioInputCommand::Stop);
  }
}

impl Drop for AudioInput {
  fn drop(&mut self) {
    debug!("audio input dropped");
    self.stop();
  }
}

pub struct AudioInputEventLoop {
  device_sample_rate: u32,
  device_channels: u16,
  /// 20ms of audio at device config (NOT guranteed to be 48khz)
  device_frame_size: usize,
  frame_size_ms: usize,
  /// 20ms of audio at 48khz
  encoder_frame_size: usize,
  sample_rate: u32,
  channels: u16,
  /// this is set initially and updated on change so it's always latest value
  preferred_mic: Option<String>,
  enabled: bool,
  commands_reciever: Receiver<AudioInputCommand>,
  commands_sender: Sender<AudioInputCommand>,
  engine_sender: Sender<PeerChannelCommand>,
  encoder_processor: EncoderProcess,
  processor_buffer: Vec<f32>,
  /// Used for single channel pop and then added to main buffer to avoid single poping from ring buffer
  resampler_pre_buffer: Vec<f32>,
  resampler_buffer: Vec<f32>,
  encoder_buffer: Vec<f32>,
  muted_vec: Vec<f32>,
  resampler: Option<Resampler>,
  bytes_output: Vec<u8>,
  encoder: opus::Encoder,
  device_stop_sender: Option<Sender<()>>,
  audio_ring_buff_producer: HeapProducer<f32>,
  audio_ring_buff_consumer: HeapConsumer<f32>,
  consumer: Option<HeapConsumer<f32>>,

  rtp_time: MediaTime,
  pool: BytesPool,
  max_bytes_output_len: usize,
  capture_delay_ms: Arc<AtomicU64>,
}

/// Audio input device stack
/// Capture device, encoder, resampler, and processor
///
/// ## First attempt
/// Moved to a thread
/// Have a (ring?)buffer, put audio through the whole stack and add to buffer
/// then have a "get_samples" that pops 10ms of audio out of our buffer
/// then call that from the engine event loop
impl AudioInputEventLoop {
  pub fn new(
    AudioInputOptions {
      echo_processor,
      enabled,
      preferred_mic,
      engine_sender,
    }: AudioInputOptions,
    commands_sender: Sender<AudioInputCommand>,
    commands_reciever: Receiver<AudioInputCommand>,
  ) -> Self {
    let channels = 2;
    let frame_size_ms = 20;
    let max_frame_size = get_opus_samples_count(48_000, channels, 60); // 60ms of 48khz streo (just for simplicity)
    let encoder_frame_size = get_opus_samples_count(
      48_000,
      channels,
      frame_size_ms
        .try_into()
        .expect("failed to convert frame size"),
    );
    let muted_vec = vec![0.0_f32; max_frame_size];
    let max_bytes_output_len = max_frame_size * 6;
    // encoder output
    let bytes_output: Vec<u8> = vec![0; max_bytes_output_len];
    // Must operate at 48khz streo to match our speaker part
    let encoder_processor = EncoderProcess::new(echo_processor, 2, 48_000);
    let processor_buffer = encoder_processor.preallocate_buffer();
    let encoder_buffer = vec![0.0_f32; encoder_frame_size]; // final audio buffer that feeds encoder
    let resampler_buffer = vec![0.0_f32; max_frame_size];
    let resampler_pre_buffer = vec![0.0_f32; max_frame_size / 2]; // one channel
                                                                  // let encoder = Arc::new(Mutex::new(Self::create_encoder()));

    let (audio_ring_buff_producer, audio_ring_buff_consumer) =
      HeapRb::<f32>::new(max_frame_size).split();

    let encoder = Self::create_encoder();

    // callibrate
    if !enabled {
      encoder_processor.set_output_will_be_muted(true);
    }

    let rtp_time = MediaTime::ZERO;

    // Create a pool for encoder output  (new)
    let pool = bytes_pool::BytesPool::new(200, max_bytes_output_len);

    Self {
      device_sample_rate: 0,
      device_channels: 0,
      device_frame_size: 0,
      frame_size_ms,
      rtp_time,
      encoder_frame_size,
      sample_rate: 48_000,
      channels,
      preferred_mic,
      enabled,
      commands_reciever,
      commands_sender,
      engine_sender,
      encoder_processor,
      processor_buffer,
      resampler_pre_buffer,
      resampler_buffer,
      encoder_buffer,
      audio_ring_buff_producer,
      audio_ring_buff_consumer,
      muted_vec,
      resampler: None,
      encoder,
      bytes_output,
      device_stop_sender: None,
      consumer: None,
      pool,
      max_bytes_output_len,
      capture_delay_ms: Arc::new(AtomicU64::new(0)),
    }
  }

  fn get_bytes_mut(&mut self) -> BytesMut {
    self.pool.get_bytes_mut()
  }

  pub async fn run(&mut self) -> Result<(), anyhow::Error> {
    let (stop_sender, stop_reciever) = flume::bounded::<()>(1);

    // ... on a thread that will compute audio and has to be real-time: buffer_size 0 will auto select from sample rate
    let prio_handle = match promote_current_thread_to_real_time(0, 48_000) {
      Ok(h) => Some(h),
      Err(e) => {
        eprintln!("Error promoting player thread to real-time: {}", e);
        None
      }
    };

    // Spawn input device thread
    let Ok(mut stream) = self.start_device().await else {
      return Err(anyhow::anyhow!("failed to start mic device"));
    };

    // let mut interval = tokio::time::interval(Duration::from_millis(self.frame_size_ms as u64)); // we operate in 20ms chunks
    // interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
      tokio::select! {
        _ = stop_reciever.recv_async() => {
          info!("input: stop signal received");
          break;
        }

        // _ = interval.tick() => {
        //   self.audio_tick().await;
        // }

        // Listen for command
        Ok(command) = self.commands_reciever.recv_async() => {
          match command {
            AudioInputCommand::GotNewAudio => {
              self.process_audio().await;
            }
            AudioInputCommand::Toggle(is_enabled) => {
              self.enabled = is_enabled;
              self.encoder_processor.set_output_will_be_muted(!is_enabled);
              if is_enabled {
                self.clear_buffer();
              }
            }
            AudioInputCommand::ChangeDevice(device) => {
              self.preferred_mic = Some(device);
              let prev_stream = stream;
              stream = self.start_device().await.expect("to start device");
              drop(prev_stream);
            }
            AudioInputCommand::Stop => {
              let _ = self.device_stop_sender
                .take()
                .expect("to have device stop sender")
                .try_send(());
                // .expect("to send device stop signal");
              let _ = stop_sender.try_send(());
            }
          }
        },
      }
    }

    info!("input: stopped");

    if let Some(handle) = prio_handle {
      let _ = demote_current_thread_from_real_time(handle);
    }

    drop(stream);

    Ok(())
  }

  fn clear_buffer(&mut self) {
    let c = self.consumer.as_mut().expect("to have consumer");
    c.skip(c.len());
    info!("input: cleared buffer");
  }

  async fn start_device(&mut self) -> anyhow::Result<cpal::Stream> {
    // let buffer_size = 240;
    let _buffer_size = 240; // for final output after resampling to be close to 480;
    let buffer_size = 480; // for better sync with echo cancel
                           // min or 512 or default if None
    let (device, config, sample_rate, channels) =
      find_input_device(self.preferred_mic.clone(), buffer_size as u32)?;

    self.device_sample_rate = sample_rate;
    self.device_channels = channels;
    self.device_frame_size = self.get_device_frame_size();

    // to reduce delay issue changed from 1.9
    let _sample_multiplier = 2.1_f32;
    let sample_multiplier = 4_f32; // increase because I increased changed buffer size

    let ring_buffer_size =
      (self.device_frame_size as f32 * sample_multiplier).ceil() as usize + buffer_size;

    let (producer, consumer) = HeapRb::<f32>::new(ring_buffer_size).split();
    let (device_stop_sender, _device_stop_reciever) = flume::bounded::<()>(1);
    if let Some(prev_stop) = self.device_stop_sender.take() {
      let _ = prev_stop.try_send(());
    }
    self.consumer = Some(consumer);
    self.device_stop_sender = Some(device_stop_sender);
    let commands_sender_clone = self.commands_sender.clone();

    //.
    // thread::spawn(move || {
    // ... on a thread that will compute audio and has to be real-time:
    // match promote_current_thread_to_real_time(buffer_size, sample_rate) {
    //   Ok(_h) => {
    //     println!("audio_input thread is now bumped to real-time priority.");
    //   }
    //   Err(e) => {
    //     eprintln!("Error promoting audio_input to real-time: {}", e);
    //   }
    // }

    let stream = start_audio_input(
      device,
      config,
      producer,
      commands_sender_clone,
      self.encoder_processor.get_echo_processor().clone(),
      self.capture_delay_ms.clone(),
    )
    .expect("to start input device");

    // keep alive
    //   while device_stop_reciever.recv().is_ok() {
    //     sleep(std::time::Duration::from_millis(1));
    //   }
    // });

    // resample to 48khz stereo before processing by echo canceller
    let resampler = Resampler::new(ResamplerConfig {
      input_sample_rate: self.device_sample_rate,
      output_sample_rate: 48_000,
      channels: 2,
      chunk: resampler::Chunk::FiveMs,
      // chunk: resampler::Chunk::TenMs,
    });
    self.resampler = Some(resampler);

    Ok(stream)
  }

  // Process 10ms of audio
  async fn process_audio(&mut self) {
    assert!(self.device_frame_size != 0, "input frame size is 0");
    assert!(self.sample_rate != 0, "input sample_rate is 0");
    assert!(self.channels != 0, "input channel is 0");

    while let Some(samples_count) = self.get_audio() {
      // expect 10ms
      let expected_final_sample_count = self.encoder_frame_size / 2;
      let input: &[f32];

      // let Some(samples_count) = samples_count else {
      //   // not enough audio data
      //   return;
      // };

      trace!("process audio got audio samples {}", samples_count);

      // resample
      let resampler = self.resampler.as_mut().expect("to have resampler");
      // debug!("to resample samples_count= {}", &samples_count);
      let resampled = resampler.process(&self.resampler_buffer[..samples_count]);
      // debug!("resampled samples_count= {}", &resampled.len());

      // put it for processor
      self.processor_buffer[0..resampled.len()].copy_from_slice(resampled);

      // Fix output size to avoid resampler diff crash
      if resampled.len() < expected_final_sample_count {
        let diff = expected_final_sample_count - resampled.len();
        let end = resampled.len() + diff;
        self.processor_buffer[resampled.len()..end].copy_from_slice(&self.muted_vec[..diff]);
      }
      let len = expected_final_sample_count;

      // feed the processor the mic output regardless of being muted to fix echo cancel bug on mic toggle
      // process
      // let size = self
      //   .encoder_processor
      //   .process(&mut self.processor_buffer[0..len])
      //   .expect("failed to process");
      let input_mut = &mut self.processor_buffer[0..len];
      let total_samples = input_mut.len();
      let size = total_samples;
      // we have to cut it in splits
      let frame_size = self.encoder_processor.frame_size();
      let frames = input_mut.chunks_exact_mut(frame_size);
      let _single_samples: usize = 0;

      // Delay
      let capture_delay_ms = self.capture_delay_ms.load(Ordering::Relaxed);

      for (_i, frame) in frames.enumerate() {
        // set estimate for frame delay ms (based on next frame expectations)
        // get delay here because it is different for each frame
        // DO NOT ADD OFFSET AS we give both frames to render at the same time
        // let less_delay_us = per_frame_delay_us * i as u64;

        trace!("processing capture frame");

        // process
        self
          .encoder_processor
          .get_echo_processor_mut()
          .process_capture_frame(frame, capture_delay_ms) //  - less_delay_us
          .expect("to process capture frame");
      }

      // assign as "input" for next step
      input = &mut self.processor_buffer[..size];

      if input.is_empty() {
        return;
      }

      // // maybe we can avoid this copy by using the processor_buffer
      // let dest = self.audio_buffer.extend_from_slice(other)
      self.audio_ring_buff_producer.push_slice(input);
    }

    // see if we can tick in encoder (when we have 20ms of data)
    self.audio_tick().await;
  }

  /// Pop 20ms of audio from the audio buffer, encode and send it
  async fn audio_tick(&mut self) {
    // check if we have enough data for a 20ms frame
    if self.audio_ring_buff_consumer.len() < self.encoder_frame_size {
      // not enough data yet
      return;
    }

    let mut data_bytes = { self.get_bytes_mut() };
    let audio_frame = &mut self.encoder_buffer[..self.encoder_frame_size];
    // clear?
    self.audio_ring_buff_consumer.pop_slice(audio_frame);
    let _len = self.encoder_frame_size;
    let is_mic_enabled = self.enabled;
    let stats = self.encoder_processor.stats();

    let input: &[f32];
    if is_mic_enabled {
      // assign as "input" for next step
      input = audio_frame
    } else {
      input = &mut self.muted_vec[..self.encoder_frame_size];
    }

    if input.is_empty() {
      warn!("input is empty");
      return;
    }

    // From here we have 10ms of audio at 48khz stereo
    let sample_count = input.len() as u64;
    let duration_ms =
      Duration::from_millis((sample_count / self.channels as u64) * 1000 / self.sample_rate as u64);
    let duration = MediaTime::new(
      sample_count / self.channels as u64,
      Frequency::FORTY_EIGHT_KHZ,
    );

    assert!(
      self.frame_size_ms as u128 == duration_ms.as_millis(),
      "frame size and duration not match expected: {} actual: {}",
      self.frame_size_ms,
      duration_ms.as_millis()
    );

    let len = self
      .encoder
      .encode_float(input, data_bytes.as_mut())
      .expect("to encode");

    data_bytes.resize(len, 0);

    // skip 1-2 bytes or less for DTX
    if len < 3 {
      return;
    }

    // get current time (starts at zero so we use last one)
    let current_rtp_time = self.rtp_time;
    // update rtp time for next packet
    self.rtp_time = current_rtp_time + duration;

    // Send to all active tracks
    self.send(
      data_bytes.freeze(),
      current_rtp_time,
      MediaExtra::Audio {
        voice_activity: stats.has_voice.unwrap_or(true),
      },
    );
  }

  /// Send to all local tracks
  fn send(&self, data: PoolBytesVec, rtp_time: MediaTime, extra: MediaExtra) {
    let _ = self.engine_sender.try_send(PeerChannelCommand::SendMedia {
      kind: MediaKind::Audio,
      data,
      rtp_time,
      extra: Some(extra),
    });
  }

  fn current_buffer_delay_us(&self) -> u64 {
    let buf_len = self.consumer.as_ref().expect("to have consumer").len();
    // this is wrong as we make it always 2 before pop
    let channels = self.device_channels;

    ((1e6 * buf_len as f64 / self.device_sample_rate as f64 / channels as f64) + 0.5) as u64
  }

  // get 10ms of raw audio from HAL ring buffer
  fn get_audio<'a>(&'a mut self) -> Option<usize> {
    let device_10ms_frame_size = self.device_frame_size / 2;

    let consumer = self.consumer.as_mut().expect("to have consumer");
    if consumer.len() < device_10ms_frame_size {
      return None;
    }

    let samples_count;
    // ensure streo
    if self.device_channels == 1 {
      // frame size is for a single channel because of device_channels = 1;
      samples_count = device_10ms_frame_size * 2;

      let mut i = 0;

      // Pop once into a pre buffer to avoid single pop as adviced against by ring buffer crate
      consumer.pop_slice(&mut self.resampler_pre_buffer[0..device_10ms_frame_size]);

      while i < samples_count {
        let sample = self.resampler_pre_buffer[i / 2];
        self.resampler_buffer[i] = sample;
        self.resampler_buffer[i + 1] = sample;
        i += 2;
      }
    } else {
      samples_count = device_10ms_frame_size;
      consumer.pop_slice(&mut self.resampler_buffer[0..samples_count]);
    }

    Some(samples_count)
  }

  fn get_device_frame_size(&self) -> usize {
    get_opus_samples_count(
      self.device_sample_rate,
      self.device_channels,
      self.frame_size_ms as u32,
    )
  }

  fn create_encoder() -> opus::Encoder {
    let mut encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)
      .expect("to create encoder");
    encoder.set_inband_fec(true).expect("to set fec");
    encoder.set_packet_loss_perc(20).expect("to set loss perc");
    encoder.set_complexity(9).expect("to set complexity");
    // it's advised to use at least 32-64kbps for streo voice
    // encoder
    //   .set_bitrate(opus::Bitrate::Bits(80_000))
    //   .expect("to set bitrate");
    encoder
  }
}

fn find_input_device(
  preferred_mic: Option<String>,
  buffer_size: u32,
) -> Result<(cpal::Device, cpal::StreamConfig, u32, u16), AudioError> {
  let preferred_mic_clone = preferred_mic.clone();
  let (device, config) = get_input_device(48_000, preferred_mic, buffer_size)?;
  let channels = config.channels;
  let sample_rate = config.sample_rate;

  info!(
    "picked input device: {} config: {:?} preferred: {:?}",
    device.name().expect("to have device name"),
    config,
    &preferred_mic_clone
  );

  Ok((device, config, sample_rate.0, channels))
}

fn start_audio_input(
  device: cpal::Device,
  config: cpal::StreamConfig,
  mut producer: HeapProducer<f32>,
  command_sender: Sender<AudioInputCommand>,
  _processor: AudioEchoProcessor,
  capture_delay_ms: Arc<AtomicU64>,
) -> anyhow::Result<cpal::Stream, anyhow::Error> {
  let sample_rate = config.sample_rate.0;
  let channels = config.channels;

  // 10ms
  // assume cpal::SampleFormat::F32
  let stream = device.build_input_stream(
    &config,
    move |data: &[f32], info| {
      // Delay
      let capture_time_ns = info.timestamp().callback.as_nanos();
      let now_host_time = cap::cidre::cv::current_host_time();
      let now_time_ns = host_time_to_stream_instant(now_host_time).as_nanos();
      let device_latency_us = 1e-3 * (now_time_ns - capture_time_ns) as f64;

      let buffer_latency_us =
        1.0e6 * producer.len() as f64 / sample_rate as f64 / channels as f64 + 0.5;

      let capture_total_delay = 1e-3 * (buffer_latency_us + device_latency_us as f64) + 0.5;
      capture_delay_ms.store(capture_total_delay as u64, Ordering::Relaxed);

      // Write
      if producer.free_len() >= data.len() {
        producer.push_slice(data);
      } else {
        error!("producer.push_slice failed, clearing buffer...");
        // do not clear here, it causes fix audio glitch
      }

      let _ = command_sender.try_send(AudioInputCommand::GotNewAudio);
    },
    cpal_err_fn,
  )?;

  stream.play()?;

  info!("input: started getting device samples...");

  // no need to keep it running, it's already in a loop
  Ok(stream)
}
