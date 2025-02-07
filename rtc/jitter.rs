use std::{
  collections::VecDeque,
  sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
  },
};

use opus::Decoder;
use serde_json::json;
use std::fmt;
use str0m::media::{Frequency, MediaData};
use systemstat::Duration;
use tokio::time::Instant;

use crate::{
  debugger::{Debugger, StatKind},
  rtc::resampler::{Chunk, Resampler, ResamplerConfig},
};

use super::{audio::opus_channels, utils::UserId};

pub struct JitterBufConfig {
  pub sample_rate: u32,
  pub channels: u16,
  pub debugger: Debugger,
  pub user_id: UserId,
}

impl fmt::Debug for JitterBufConfig {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("JitterBufConfig")
      .field("sample_rate", &self.sample_rate)
      .field("channels", &self.channels)
      .field("user_id", &self.user_id)
      .finish()
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Status {
  /// No packets received yet
  WaitingForFirstPacket,
  /// First packet received, waiting for more
  WaitingForMorePackets,
  /// Enough packets received, ready to play
  Normal,
  /// Buffer is empty, increasing buffer to reach min buffer
  IncreasingBuffer,
  /// Buffer is full, decreasing buffer to reach max buffer
  DecreasingBuffer,
}

pub struct JitterBuffer {
  sample_rate: u32,
  channels: u16,
  packets: Arc<Mutex<VecDeque<Arc<MediaData>>>>,
  decoder: Arc<Mutex<Decoder>>,
  // sample_builder: Arc<Mutex<SampleBuilder<OpusPacket>>>,
  audio_buffer: Arc<Mutex<VecDeque<f32>>>,
  decoder_output_buffer: Arc<Mutex<Vec<f32>>>,
  /// If sample builder couldn't fix missing packet, used to decode FEC in OPUS
  previous_lost: AtomicBool,
  current_tick: AtomicU64,
  // "ptime" ms
  packet_duration: Arc<AtomicU32>,
  total_lost: Arc<AtomicU64>,
  total_packets: Arc<AtomicU64>,
  total_fec: Arc<AtomicU64>,
  curr_instant: Arc<Mutex<Instant>>,
  status: Arc<Mutex<Status>>,
  slow_speed_resampler: Arc<Mutex<Resampler>>,
}

const PACKET_DURATION_MS: u16 = 20;
// const MAX_LATE_MS: u16 = 100;
// const MIN_BUFFER_MS: usize = 2000;
// const MAX_BUFFER_MS: usize = 2400;

// static jitter buffer
const MIN_BUFFER_MS: usize = 100; // prev: 80
const MAX_BUFFER_MS: usize = 1000; // prev: 500
const ACCELERATE_WHEN_PACKETS_HELD_BACK: usize = 20; // prev: 15
const CAP_PACKETS_HELD_BACK: usize = 80; // prev: 15
pub const MAX_10MS_SAMPLE_COUNT: usize = 10 /* ms */ * (48_000 / 1000) * 2 /* channels */;

impl Drop for JitterBuffer {
  fn drop(&mut self) {
    info!("jitter buffer dropped.");
  }
}

impl JitterBuffer {
  pub fn new(config: JitterBufConfig) -> Self {
    let decoder = Arc::new(Mutex::new(
      opus::Decoder::new(config.sample_rate, opus_channels(config.channels))
        .expect("failed to create decoder"),
    ));

    // Pre-alloc sync buffer for 2000ms of audio
    let audio_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(
      MAX_10MS_SAMPLE_COUNT * (MAX_BUFFER_MS / 10),
    )));
    // must be filled with 0.0
    let decoder_output_buffer = Arc::new(Mutex::new(vec![0.0f32; MAX_10MS_SAMPLE_COUNT * 2]));
    let previous_lost = AtomicBool::new(false);
    let packet_duration = Arc::new(AtomicU32::new(0));
    let current_tick = AtomicU64::new(0);
    let total_lost = Arc::new(AtomicU64::new(0));
    let total_packets = Arc::new(AtomicU64::new(0));
    let total_fec = Arc::new(AtomicU64::new(0));
    let status = Arc::new(Mutex::new(Status::WaitingForFirstPacket));

    // used for 0.5x
    let slow_speed_resampler = Arc::new(Mutex::new(Resampler::new(ResamplerConfig {
      input_sample_rate: config.sample_rate,
      output_sample_rate: (config.sample_rate as f32 / 0.75).floor() as u32,
      channels: config.channels,
      chunk: Chunk::TwoAndHalfMs,
    })));

    // it was set at 100 causing way too much delay
    // packet size is 20ms
    // 300 / 20 = 30
    let packets = Arc::new(Mutex::new(VecDeque::with_capacity(CAP_PACKETS_HELD_BACK)));

    info!("New Jitter Buffer - config={:#?}", &config);

    let audio_buffer_clone = Arc::downgrade(&audio_buffer);
    let decoder_clone = Arc::downgrade(&decoder);
    let debugger_clone = config.debugger.clone();
    let packet_duration_clone = packet_duration.clone();
    let total_lost_clone = total_lost.clone();
    let total_packets_clone = total_packets.clone();
    let total_fec_clone = total_fec.clone();
    let user_id_clone = config.user_id.clone();
    let status_clone = status.clone();
    let packets_clone = packets.clone();

    tokio::task::spawn_local(async move {
      let debugger = debugger_clone;
      let mut interval = tokio::time::interval(Duration::from_millis(250));
      let mut log_to_console = 0;
      while let Some(audio_buffer) = audio_buffer_clone.upgrade() {
        log_to_console += 1;
        interval.tick().await;
        let buffer_len = { audio_buffer.as_ref().lock().expect("poisened").len() };
        let (bw, lpd, smpr) = {
          if let Some(d) = decoder_clone.upgrade() {
            let mut decoder = d.as_ref().lock().expect("poisened");
            (
              decoder.get_bandwidth().unwrap_or(opus::Bandwidth::Auto),
              decoder.get_last_packet_duration().unwrap_or(1),
              decoder.get_sample_rate().unwrap_or(1),
            )
          } else {
            (opus::Bandwidth::Auto, 1, 1)
          }
        };

        let status = { *status_clone.lock().expect("to have status") };
        let packet_len = { packets_clone.lock().expect("to get packets len").len() };
        debugger.stat(
          StatKind::PacketDurationMs,
          json!(packet_duration_clone.load(Ordering::Relaxed)),
          Some(&user_id_clone),
        );
        debugger.stat(
          StatKind::JitterPacketBuffer,
          json!(packet_len),
          Some(&user_id_clone),
        );
        debugger.stat(
          StatKind::JitterStatus,
          json!(format!("{:#?}", status)),
          Some(&user_id_clone),
        );
        debugger.stat(
          StatKind::FECCount,
          json!(total_fec_clone.load(Ordering::Relaxed)),
          Some(&user_id_clone),
        );
        debugger.stat(
          StatKind::PacketLoss,
          json!(total_lost_clone.load(Ordering::Relaxed)),
          Some(&user_id_clone),
        );
        debugger.stat(
          StatKind::PacketReceived,
          json!(total_packets_clone.load(Ordering::Relaxed)),
          Some(&user_id_clone),
        );
        debugger.stat(StatKind::PacketDuration, json!(lpd), Some(&user_id_clone));
        debugger.stat(
          StatKind::JitterBufferSamples,
          json!(buffer_len),
          Some(&user_id_clone),
        );

        if log_to_console % 16 == 0 {
          // only log ever 4s in console
          debug!(
            "jitter stats: audio_buffer_len={}  bandwidth={:?} last_packet={}  sample_rate={}",
            buffer_len, bw, lpd, smpr
          );
        }
      }
    });

    Self {
      sample_rate: config.sample_rate,
      channels: config.channels,
      decoder,
      // sample_builder,
      packets,
      audio_buffer,
      decoder_output_buffer,
      packet_duration,
      previous_lost,
      current_tick,
      total_lost,
      total_packets,
      total_fec,
      curr_instant: Arc::new(Mutex::new(Instant::now())),
      status,
      slow_speed_resampler,
    }
  }

  // new methods
  pub fn insert_packet(&self, p: Arc<MediaData>) {
    let _timestamp = p.time.rebase(Frequency::MILLIS); // MILLIS

    // debug!(
    //   "packet inserted time={} va={:#?} seq={:#?}-{:#?} nettime={}ms",
    //   p.time.as_ntp_32(),
    //   p.ext_vals.voice_activity.unwrap_or(false),
    //   p.seq_range.start(),
    //   p.seq_range.end(),
    //   p.network_time.elapsed().as_millis()
    // );
    // debug!("incoming packet {:?} now {:?}", &p, &timestamp);

    // Push to sample_builder
    // self.sample_builder.lock().expect("poisened").push(p);
    {
      let mut packets = self.packets.lock().expect("poisened");

      // Clear except for 1 packets
      if packets.len() > CAP_PACKETS_HELD_BACK {
        debug!("max packets held back, clearing except 1");
        for _ in 0..CAP_PACKETS_HELD_BACK {
          let _ = packets.pop_front();
        }
        trace!("len after = {}", &packets.len());
      }

      // push the new packet
      packets.push_back(p);
    }

    if self.status() == Status::WaitingForFirstPacket {
      // got first packet, store it we use it in tick()
      self.set_status(Status::WaitingForMorePackets);
    }

    // after getting first packet, start tick skipping to accumolate buffer
    //
  }

  pub fn get_packets_len(&self) -> usize {
    self.packets.lock().expect("poisened").len()
  }

  /// Get 10ms of audio
  pub fn get_audio(&self, output: &mut [f32]) -> usize {
    let required_initial_buffer_amount = MIN_BUFFER_MS / 10 * self.sample_count_10ms();
    let status = self.status();
    if status == Status::WaitingForFirstPacket || status == Status::WaitingForMorePackets {
      if self.audio_buffer.lock().expect("poisened").len() > required_initial_buffer_amount {
        self.set_status(Status::Normal);
      } else {
        // wait for a bit of audio buffer
        return self.generate_silence(1, output);
      }
    }

    self.pop_audio(output)
  }

  /// Called every 20ms after first packet
  pub fn tick(&self) {
    // on each tick, check if we have a sample to decode
    // 1. if has a valid sample: decode, add to audio buffer
    // 2. if packet loss: mark as has packet loss, so we decode FEC in the next tick
    // 3. if we have a full audio buffer,

    if self.status() == Status::WaitingForFirstPacket {
      trace!("waiting for first packet");
      // no ticks yet until we get our first packet, then we buffer a bit and start playing
      return;
    }

    if let Some(sample) = { self.packets.lock().expect("to have packets").pop_front() } {
      // if let Some(sample) = { self.packets.lock().expect("po").pop_back() } {
      // if let Some(sample) = self.pop() {
      let mut decoder_output_buffer = self
        .decoder_output_buffer
        .lock()
        .expect("to have decoder output buffer");
      let mut decoder = self.decoder.lock().expect("posiened");

      // self.trace(&sample, &decoder);
      if sample.data.len() < 3 {
        trace!("got empty sample: {:#?}", sample);
      }

      // if pev lost, decode extra before
      // let adjusted_dropped = Self::adjusted_packet_drop(&sample);
      let (last_packet_dur, last_packet_samples) = self.last_packet_duration_ms(&mut decoder);

      // if self.was_previous_lost() && adjusted_dropped > 0 {
      if self.was_previous_lost() {
        trace!(
          "detected prev packet loss, last_packet_dur={}ms",
          last_packet_dur
        );

        // Decode FEC
        trace!("last_packet_samples={}", &last_packet_samples);
        let result = decoder.decode_float(sample.data.as_ref(), &mut decoder_output_buffer, true);
        /* for packet loss, pass &[] + FEC = 1 */

        // info!("packet loss decoding logs below:");
        if let Ok(total_samples) = self.push_decoder_result(
          &result,
          &decoder_output_buffer,
          last_packet_samples as usize,
          last_packet_dur,
        ) {
          trace!("pushed FEC decoded data to audio buffer {}", total_samples);
          self.total_fec.fetch_add(1, Ordering::Relaxed);
        }

        // done
        self.clear_mark_as_lost();
      }

      // decode
      let packet = sample.data.as_ref();

      let result = decoder.decode_float(packet, &mut decoder_output_buffer, false);
      let sample_count = if let Ok(samples) = &result {
        // 960
        samples
      } else {
        &0
      };

      /* this time FEC = false */
      let _ = self.push_decoder_result(
        &result,
        &decoder_output_buffer,
        *sample_count,
        last_packet_dur,
      );

      // Save packet duration
      if *sample_count > 0 {
        self.save_p_dur_from_samples(sample_count);
      }

      self.total_packets.fetch_add(1, Ordering::Relaxed);
    } else {
      // mark as lost (so in the next packet we'll try decoding)
      self.mark_as_lost();
      self.total_lost.fetch_add(1_u64, Ordering::Relaxed);
    }

    // increase tick
    self.current_tick.fetch_add(1, Ordering::Relaxed);
  }

  // UTILS
  fn save_p_dur_from_samples(&self, nb_samples: &usize) {
    let packet_duration = self.samples_to_ms(*nb_samples as u32);
    self
      .packet_duration
      .store(packet_duration as u32, Ordering::Relaxed)
  }
  fn current_tick(&self) -> u64 {
    self.current_tick.load(Ordering::Relaxed)
  }
  fn status(&self) -> Status {
    *self.status.lock().expect("to have status")
  }
  fn set_status(&self, status: Status) {
    let mut s = self.status.lock().expect("to have status");
    *s = status;
  }
  fn was_previous_lost(&self) -> bool {
    self.previous_lost.load(Ordering::Relaxed)
  }
  fn mark_as_lost(&self) {
    self.previous_lost.store(true, Ordering::Relaxed);
  }
  fn clear_mark_as_lost(&self) {
    self.previous_lost.store(false, Ordering::Relaxed);
  }
  fn samples_to_ms(&self, sample_count_per_chan: u32) -> usize {
    // sample_count as usize / (self.sample_rate as usize / 1000 * self.channels as usize)
    sample_count_per_chan as usize / (self.sample_rate as usize / 1000)
  }

  /// Gets last packet duration samples and converts to ms, if not present, returns 20ms default
  fn last_packet_duration_ms(&self, decoder: &mut opus::Decoder) -> (usize, u32) {
    let last_packet_samples = decoder.get_last_packet_duration().unwrap_or(0);
    (
      // find duration in ms
      if last_packet_samples == 0 {
        PACKET_DURATION_MS as usize
      } else {
        self.samples_to_ms(last_packet_samples)
      },
      last_packet_samples,
    )
  }

  fn push_decoder_result(
    &self,
    result: &Result<usize, opus::Error>,
    decoder_output_buffer: &[f32],
    samples_per_chann: usize,
    _last_packet_dur_ms: usize,
  ) -> Result<usize, ()> {
    if let Err(err) = result {
      error!("failed to decode {:?}", err);
      trace!("did not push silence when FEC failed");
      // DO NOT ADD
      // add silence?
      // self.push_audio(&self.generate_silence(last_packet_dur_ms / 10));
      // trace!("pushed silence (bc failed to decode)");
      return Err(());
    // } else if let Ok(sample_count_per_chan) = result {
    } else if result.is_ok() {
      // sample_count from decode is per channel
      let total_samples = samples_per_chann * self.channels as usize;
      self.push_audio(&decoder_output_buffer[0..total_samples]);
      return Ok(total_samples);
    }

    Err(())
  }

  fn push_audio(&self, audio_frame: &[f32]) {
    let mut audio_buffer = self.audio_buffer.lock().expect("poisened");
    if audio_buffer.len() + audio_frame.len() > audio_buffer.capacity() {
      // not enough capacity
      // flush from new packets in back to skip a bit
      audio_buffer.drain(0..audio_frame.len());
      trace!("flushed audio buffer in jitter buffer");
    }

    for sample in audio_frame {
      audio_buffer.push_back(*sample);
    }
  }

  fn pop_audio(&self, output: &mut [f32]) -> usize {
    let mut audio_buffer = self.audio_buffer.lock().expect("poisened");
    // if audio_buffer.len() >= self.sample_count_10ms() {
    // trace!("pop_audio has_buffer={}", audio_buffer.len());

    let packet_len = self.get_packets_len();
    let buffer_len = audio_buffer.len();
    let samples_10ms = self.sample_count_10ms();
    // let is_over_buffer = buffer_len > (MAX_BUFFER_MS / 10) * samples_10ms;
    let has_too_many_packets = packet_len > ACCELERATE_WHEN_PACKETS_HELD_BACK;
    let is_over_buffer =
      buffer_len > ((MAX_BUFFER_MS / 10 - 4) * samples_10ms) && has_too_many_packets;
    let is_under_buffer = buffer_len < (MIN_BUFFER_MS / 10) * samples_10ms;
    let is_buffer_normal = !is_over_buffer && !is_under_buffer;
    let is_critically_under_buffer = buffer_len < samples_10ms; // this is when we hit 0 and switch to IncreasingBuffer
    let current_status = self.status();
    let next_status = if current_status == Status::IncreasingBuffer && is_buffer_normal {
      Status::Normal
    } else if current_status == Status::DecreasingBuffer && is_buffer_normal {
      Status::Normal
    } else if current_status == Status::Normal && is_over_buffer {
      Status::DecreasingBuffer
    // } else if current_status == Status::Normal && is_under_buffer {
    } else if current_status == Status::Normal && is_critically_under_buffer {
      Status::IncreasingBuffer
      // Status::Normal // Do not do anything yet
    } else {
      // don't touch it
      current_status
    };

    // save status
    self.set_status(next_status);

    if next_status == Status::IncreasingBuffer {
      trace!("increasing buffer by 10ms");
      // silence until we've buffer min amount
      return self.generate_silence(1, output);
    }

    let playback_speed: f32 = if next_status == Status::DecreasingBuffer {
      2.0 // 20ms fast forward
    } else if next_status == Status::IncreasingBuffer {
      // 0.75 // 5ms slow down
      1.0 // 5ms slow down
    } else {
      1.0 // 10ms normal
    };

    if playback_speed != 1.0 {
      debug!("play {}x speed, buffer={}", playback_speed, buffer_len)
    }

    // have audio data
    let sample_count = (samples_10ms as f32 * playback_speed).floor() as usize;

    let mut output_i = 0;
    // let mut output = Vec::with_capacity(sample_count);
    for i in 0..sample_count / self.channels as usize {
      // loop over left/right samples in interleaved
      if playback_speed == 2.0 && i % 2 == 0 {
        // every other sample, discard;
        for _ in 0..self.channels {
          let _ = audio_buffer.pop_front();
        }
      } else {
        // sample / channel
        for _j in 0..self.channels {
          output[output_i] = audio_buffer.pop_front().expect("sample in buff");
          output_i += 1;
          // output.push(audio_buffer.pop_front().expect("sample in buff"));
        }
      }
    }

    drop(audio_buffer);

    if playback_speed == 0.75 {
      let mut resampler = self
        .slow_speed_resampler
        .lock()
        .expect("to have slow resampler");
      let slow_output = { resampler.process(output) };

      output.copy_from_slice(slow_output);
      // output.clear();
      // output.extend_from_slice(slow_output);
    }

    assert!(
      output.len() == samples_10ms,
      "pop_audio did not give 10ms of samples, output.len()={} expected={}",
      output.len(),
      samples_10ms
    );

    // output
    output_i
  }

  pub fn sample_count_10ms(&self) -> usize {
    self.sample_rate as usize / 1000 * 10 * self.channels as usize
  }

  /// Allocate enough in a vec for each call to get_audio
  pub fn allocate_output_buffer(&self) -> Vec<f32> {
    let samples_in_each_tick = self.sample_count_10ms();
    vec![0.0f32; samples_in_each_tick]
  }

  fn generate_silence(&self, num_10ms_frames: usize, output: &mut [f32]) -> usize {
    let samples_count = self.sample_count_10ms() * num_10ms_frames;

    assert!(
      output.len() == samples_count,
      "output slice passed to generate silence len {} did not match {}",
      output.len(),
      &samples_count
    );

    for i in 0..samples_count {
      output[i] = 0.0f32;
    }

    // output[..samples_count] = &[0.0f32; samples_count];
    // vec![0.0f32; samples_count]
    samples_count
  }
}
