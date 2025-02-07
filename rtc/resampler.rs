use rubato::{FftFixedIn, VecResampler};

use crate::rtc::audio::get_opus_samples_count;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunk {
  TenMs,
  #[allow(unused)]
  FiveMs,
  TwoAndHalfMs,
}

pub struct ResamplerConfig {
  pub input_sample_rate: u32,
  pub output_sample_rate: u32,
  pub channels: u16,
  pub chunk: Chunk,
}

pub struct Resampler {
  resampler: FftFixedIn<f32>,
  // resampler: FftFixedInOut<f32>,
  needs_resampling: bool,
  resampler_in_buffer: Vec<Vec<f32>>,
  resampler_out_buffer: Vec<Vec<f32>>,
  output_buffer: Vec<f32>,
  /// Interleaved output for a given frame (may consist of multiple 10ms chunks appeneded in a single vec)
  chunk_size: usize,
  channels: u16,
}

impl Resampler {
  pub fn new(config: ResamplerConfig) -> Self {
    info!(
      "input_sample_rate {} output_sample_rate {}",
      &config.input_sample_rate, &config.output_sample_rate,
    );

    let samples_count_10ms = get_opus_samples_count(config.input_sample_rate, config.channels, 10);
    // 10 ms
    let chunk_size = if config.chunk == Chunk::TenMs {
      samples_count_10ms
    } else if config.chunk == Chunk::TwoAndHalfMs {
      samples_count_10ms / 4
    } else {
      samples_count_10ms / 2
    } / config.channels as usize;
    info!("resampler: input_sample_rate={}", config.input_sample_rate);
    info!("resampler: samples_count_10ms={}", samples_count_10ms);
    info!("resampler: chunk_size={}", chunk_size);
    //
    // let resampler = FftFixedOut::<f32>::new(
    let resampler = FftFixedIn::<f32>::new(
      config.input_sample_rate as usize,
      config.output_sample_rate as usize,
      // get 10ms
      chunk_size,
      2,
      config.channels as usize,
    )
    .expect("Failed to create resampler");

    let needs_resampling = config.input_sample_rate != config.output_sample_rate;

    // Pre-allocate
    let resampler_in_buffer: Vec<Vec<f32>> = resampler.input_buffer_allocate(true);
    let resampler_out_buffer: Vec<Vec<f32>> = resampler.output_buffer_allocate(true);
    // max 120ms per spec
    let output_buffer: Vec<f32> = Vec::with_capacity(samples_count_10ms * 12);

    Self {
      resampler,
      needs_resampling,
      resampler_in_buffer,
      resampler_out_buffer,
      output_buffer,
      chunk_size,
      channels: config.channels,
    }
  }

  /// Process in 10 ms chunks
  pub fn process<'a, 'b>(&'a mut self, samples: &'b [f32]) -> &'a [f32] {
    if self.needs_resampling {
      // cleanup previous frame
      self.output_buffer.clear();

      // we will furthur split this per channels before the process
      let chunk_size_before_split = self.chunk_size * self.channels as usize;
      // info!(
      //   "resampler.process: samples={} channels={} chunks={}",
      //   &samples.len(),
      //   self.channels,
      //   &samples.len() / chunk_size_before_split
      // );

      // Resample = do synchronous resampling
      let in_buffer = self.resampler_in_buffer.as_mut();
      let mut processed_samples = 0;

      for chunk in samples.chunks(chunk_size_before_split) {
        // convert to non-interleaved
        let _ = Self::process_for_resampler(chunk, self.channels, in_buffer);

        // info!("chunk len={}", chunk.len());

        // process chunk
        self
          .resampler
          .process_into_buffer(in_buffer, self.resampler_out_buffer.as_mut_slice(), None)
          .expect("resampler failed to process");

        // Insert into output buffer
        let samples_per_channel = self.resampler_out_buffer[0].len();

        trace!(
          "resampler.process: left={:?} right={:?}",
          &self.resampler_out_buffer[0][0..40],
          &self.resampler_out_buffer[1][0..40]
        );

        assert!(
          self.resampler_out_buffer[0].len() == self.resampler_out_buffer[1].len(),
          "resampler output buffer is not the same size"
        );
        assert!(self.channels == 2, "channels is not 2");

        // dbg!(samples_per_channel);
        // the order is important
        for sample_index in 0..samples_per_channel {
          for channel_index in 0..self.channels as usize {
            processed_samples += 1;
            self
              .output_buffer
              .push(self.resampler_out_buffer[channel_index][sample_index]);
          }
        }
      }

      // output all chunks as a whole
      &self.output_buffer[0..processed_samples]
    } else {
      // no need to touch
      self.output_buffer.clear();
      self.output_buffer.extend_from_slice(samples);
      &self.output_buffer[0..samples.len()]
      // return samples;
    }
  }

  /// Converts interleaved samples to non-interleaved,
  /// output how many noninterleaved vecs should be used
  fn process_for_resampler(
    input_samples: &[f32],
    channels: u16,
    processed_buffer: &mut Vec<Vec<f32>>,
  ) -> usize {
    processed_buffer[0].clear();
    processed_buffer[1].clear();

    if channels == 2 {
      for (_i, pair) in input_samples[..].chunks_exact(2).enumerate() {
        processed_buffer[0].push(pair[0]);
        processed_buffer[1].push(pair[1]);
      }
    } else {
      for sample in input_samples {
        processed_buffer[0].push(*sample);
        // added by mo to fix the single channel bug
        processed_buffer[1].push(*sample);
        // if a channel sub vec length = 0, it is marked as inactive by resampler
      }
    }

    input_samples.len() / channels as usize
  }
}
