use crate::{debugger::Debugger, rtc::audio::get_opus_samples_count};
use std::sync::Arc;
use str0m::media::MediaData;

use super::{
  jitter::{self, JitterBuffer, MAX_10MS_SAMPLE_COUNT},
  peer::channel::AudioMedia,
  resampler::{self, Resampler, ResamplerConfig},
  utils::UserId,
};

/// Track decoder
pub struct TrackDecoder {
  jitter_buffer: Arc<JitterBuffer>,
  resampler: Resampler,
  task: tokio::task::JoinHandle<()>,
  close_sender: flume::Sender<()>,
  jitter_output_buffer: Vec<f32>,
}

impl Drop for TrackDecoder {
  fn drop(&mut self) {
    let _ = self.close_sender.try_send(());
    self.task.abort();
  }
}

impl TrackDecoder {
  pub async fn new(user_id: UserId, debugger: Debugger, media: AudioMedia) -> Self {
    let jitter_buffer = JitterBuffer::new(jitter::JitterBufConfig {
      sample_rate: media.clock_rate,
      channels: media.channels,
      debugger,
      user_id,
    });
    let resampler = Resampler::new(ResamplerConfig {
      input_sample_rate: media.clock_rate,
      channels: media.channels,
      output_sample_rate: 48_000,
      chunk: resampler::Chunk::TenMs,
    });
    // We return slices of this after processing
    // let output_buffer = Vec::with_capacity(MAX_10MS_SAMPLE_COUNT * 5);
    let _output_buffer = vec![0.0f32; MAX_10MS_SAMPLE_COUNT * 2];

    let mut interval_20ms = tokio::time::interval(std::time::Duration::from_millis(20));
    interval_20ms.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let jitter_buffer = Arc::new(jitter_buffer);
    let jitter_buffer_ = jitter_buffer.clone();
    let jitter_output_buffer = jitter_buffer.allocate_output_buffer();
    let (close_sender, close_recv) = flume::bounded::<()>(1);

    // no need to be in same thread as player
    // let task = tokio::task::spawn(async move {
    let task = tokio::task::spawn_local(async move {
      loop {
        tokio::select! {
          _ = interval_20ms.tick() => {
            jitter_buffer_.tick();
          },
          _ = close_recv.recv_async() => {
            break;
          }
        }
      }
    });

    Self {
      jitter_buffer,

      resampler,
      task,
      close_sender,
      jitter_output_buffer,
    }
  }

  pub fn insert_packet(&self, media_data: Arc<MediaData>) {
    self.jitter_buffer.insert_packet(media_data);
  }
  // pub fn decoder_tick(&self) {
  //   self.jitter_buffer.tick();
  // }

  /// Outputs 10ms of streo interleaved audio at 48khz
  pub fn get_audio<'a>(&'a mut self, dest: &mut [f32]) -> usize {
    let samples = self
      .jitter_buffer
      .get_audio(self.jitter_output_buffer.as_mut_slice());
    let output = &self.jitter_output_buffer[0..samples];

    // RESAMPLE TO 48khz
    let output = self.resampler.process(output);

    // todo : optimize
    let expected = get_opus_samples_count(48_000, 2, 10);
    assert!(
      output.len() == expected,
      "track output frame size not correct. expected: {} actual: {}",
      expected,
      output.len()
    );

    // self.output_buffer[0..output.len()]
    //   .as_mut()
    //   .copy_from_slice(output);

    // &self.output_buffer[0..output.len()]

    dest[0..output.len()].as_mut().copy_from_slice(output);

    output.len()
  }
}
