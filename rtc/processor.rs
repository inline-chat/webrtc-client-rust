use crate::debugger::{Debugger, StatKind};
use std::{
  sync::{atomic::AtomicU64, Arc},
  time::Instant,
};
use webrtc_audio_processing::{
  EchoCancellationSuppressionLevel, Error, InitializationConfig, NoiseSuppressionLevel, Processor,
  NUM_SAMPLES_PER_FRAME,
};

#[derive(Clone)]
pub struct AudioEchoProcessor {
  inner: Processor,
  noise_cancel_level: NoiseSuppressionLevel,
  echo_cancel_level: EchoCancellationSuppressionLevel,
  playback_delay: Arc<AtomicU64>,
  debugger: Debugger,
}

// Notes:
// Process at 48khz and stereo

impl AudioEchoProcessor {
  pub fn new(debugger: Debugger, echo_cancel: String, noise_cancel: String) -> Self {
    let echo_cancel_level = match echo_cancel.as_str() {
      "lowest" => EchoCancellationSuppressionLevel::Lowest,
      "lower" => EchoCancellationSuppressionLevel::Lower,
      "low" => EchoCancellationSuppressionLevel::Low,
      "moderate" => EchoCancellationSuppressionLevel::Moderate,
      "high" => EchoCancellationSuppressionLevel::High,
      _ => EchoCancellationSuppressionLevel::Moderate,
    };

    let noise_cancel_level = if noise_cancel == "moderate" {
      NoiseSuppressionLevel::Moderate
    } else if noise_cancel == "high" {
      NoiseSuppressionLevel::High
    } else if noise_cancel == "very_high" {
      NoiseSuppressionLevel::VeryHigh
    } else {
      NoiseSuppressionLevel::VeryHigh
    };

    debug!("processor noise_cancel_level {:#?}", &noise_cancel_level);
    let mut processor = Self::create_processor(2, 2).expect("to create processor");

    // Initial config
    AudioEchoProcessor::set_config_static(
      &mut processor,
      None,
      noise_cancel_level,
      echo_cancel_level,
    );

    let (_playback_delay_ms_sender, _playback_delay_ms_recv) =
      flume::unbounded::<(usize, Instant)>();

    Self {
      inner: processor,
      debugger,
      noise_cancel_level,
      echo_cancel_level,
      playback_delay: Arc::new(AtomicU64::new(0)),
    }
  }

  fn create_processor(
    num_capture_channels: i32,
    num_render_channels: i32,
  ) -> Result<Processor, webrtc_audio_processing::Error> {
    let processor = Processor::new(&InitializationConfig {
      num_capture_channels: num_capture_channels,
      num_render_channels: num_render_channels,
      ..Default::default()
    })?;

    Ok(processor)
  }

  fn set_config_static(
    processor: &mut Processor,
    stream_delay_ms: Option<i32>,
    noise_cancel_level: NoiseSuppressionLevel,
    echo_cancel_level: EchoCancellationSuppressionLevel,
  ) {
    // High pass filter is a prerequisite to running echo cancellation.
    let config = webrtc_audio_processing::Config {
      echo_cancellation: Some(webrtc_audio_processing::EchoCancellation {
        suppression_level: echo_cancel_level,
        stream_delay_ms: stream_delay_ms,
        enable_delay_agnostic: false,
        enable_extended_filter: false,
      }),

      enable_transient_suppressor: true,
      enable_high_pass_filter: true,

      noise_suppression: Some(webrtc_audio_processing::NoiseSuppression {
        suppression_level: noise_cancel_level,
      }),

      gain_control: None,
      // gain_control: Some(webrtc_audio_processing::GainControl {
      //   compression_gain_db: 12,
      //   mode: webrtc_audio_processing::GainControlMode::AdaptiveDigital,
      //   target_level_dbfs: 18,
      //   enable_limiter: true,
      // }),
      voice_detection: Some(webrtc_audio_processing::VoiceDetection {
        // FIXME: calculate this based on key pressed
        detection_likelihood: webrtc_audio_processing::VoiceDetectionLikelihood::Low,
      }),

      ..webrtc_audio_processing::Config::default()
    };

    processor.set_config(config);
  }

  pub fn num_samples_per_frame(&self) -> usize {
    NUM_SAMPLES_PER_FRAME as usize
  }

  pub fn set_playback_delay_ms(&self, render_delay_ms: u64) {
    self
      .playback_delay
      .store(render_delay_ms, std::sync::atomic::Ordering::Relaxed);
    // dbg!(render_delay_us);
    // let now = Instant::now();
    // let mut pbd = self.playback_delay.lock().expect("get playback delay");
    // *pbd = (render_delay_us, now);
  }

  /// Attempt to calculate fresh delay if new estimate is available for renderer size
  /// otherwise return None and the processor will use its last used value
  pub fn update_current_stream_delay(&mut self, _capture_delay_ms: u64) {
    // Sets the delay in ms between process_render_frame() receiving a far-end frame
    // and process_capture_frame() receiving a near-end frame containing the corresponding echo.

    let stream_delay_ms = {
      // get
      let render_delay_ms = {
        self
          .playback_delay
          .load(std::sync::atomic::Ordering::Relaxed)
      };
      let total_delay_ms = render_delay_ms + render_delay_ms;
      Some(total_delay_ms)
    };

    // Avoid any unneccessary clone in the audio loop
    // #[cfg(debug_assertions)]
    {
      let stream_delay_ms_ = stream_delay_ms.clone();
      let debugger = self.debugger.clone();
      tokio::spawn(async move {
        debugger.stat(StatKind::StreamDelayMs, stream_delay_ms_.into(), None);
      });
    }

    if let Some(stream_delay_ms) = stream_delay_ms {
      Self::set_config_static(
        &mut self.inner,
        Some(stream_delay_ms as i32),
        self.noise_cancel_level,
        self.echo_cancel_level,
      )
      // self.inner.set_stream_delay_ms(stream_delay_ms as usize);
    }
  }

  pub fn set_output_will_be_muted(&self, muted: bool) {
    self.inner.set_output_will_be_muted(muted);
  }

  pub fn set_stream_key_pressed(&self, key_pressed: bool) {
    self.inner.set_stream_key_pressed(key_pressed);
  }

  pub fn get_stats(&self) -> webrtc_audio_processing::Stats {
    self.inner.get_stats()
  }

  pub fn process_capture_frame(
    &mut self,
    frame: &mut [f32],
    capture_delay_ms: u64,
  ) -> Result<(), Error> {
    // should call conditionally?
    self.update_current_stream_delay(capture_delay_ms);
    self.set_stream_key_pressed(platform_utils::key_pressed());
    self.inner.process_capture_frame(frame)
  }

  pub fn process_render_frame(&mut self, frame: &mut [f32]) {
    self
      .inner
      .process_render_frame(frame)
      .expect("to process render frame");
  }
}

// V2

// use std::{
//   sync::{Arc, Mutex},
//   time::Instant,
// };
// use webrtc_audio_processing::{
//   EchoCanceller, Error, GainController, GainControllerMode, NoiseSuppression,
//   NoiseSuppressionLevel, Pipeline, Processor,
// };

// use crate::{debugger::Debugger, macos};

// use super::audio::create_processor;

// #[derive(Clone)]
// pub struct AudioEchoProcessor {
//   inner: Processor,
//   noise_cancel_level: NoiseSuppressionLevel,
//   playback_delay: Arc<Mutex<(usize, Instant)>>,
//   debugger: Debugger,
// }

// // Process at 48khz and streo
// impl AudioEchoProcessor {
//   pub fn new(debugger: Debugger, _echo_cancel: String, noise_cancel: String) -> Self {
//     let noise_cancel_level = if noise_cancel == "moderate" {
//       NoiseSuppressionLevel::Moderate
//     } else if noise_cancel == "high" {
//       NoiseSuppressionLevel::High
//     } else if noise_cancel == "very_high" {
//       NoiseSuppressionLevel::VeryHigh
//     } else {
//       NoiseSuppressionLevel::VeryHigh
//     };

//     debug!("processor noise_cancel_level {:#?}", &noise_cancel_level);
//     let mut processor = create_processor(2, 2, noise_cancel_level).expect("to create processor");

//     // Initial config
//     AudioEchoProcessor::set_config_static(&mut processor, None, noise_cancel_level);

//     let (playback_delay_ms_sender, playback_delay_ms_recv) = flume::unbounded::<(usize, Instant)>();

//     Self {
//       inner: processor,
//       debugger,
//       noise_cancel_level,
//       playback_delay: Arc::new(Mutex::new((0, Instant::now()))),
//     }
//   }

//   // pub fn get_processor(&self) -> Processor {
//   //   self.inner.clone()
//   // }

//   fn set_config_static(
//     processor: &mut Processor,
//     _stream_delay_ms: Option<i32>,
//     noise_cancel_level: NoiseSuppressionLevel,
//   ) {
//     // High pass filter is a prerequisite to running echo cancellation.
//     let config = webrtc_audio_processing::Config {
//       echo_canceller: EchoCanceller::Full {
//         enforce_high_pass_filtering: false,
//       }
//       .into(),

//       reporting: webrtc_audio_processing::ReportingConfig {
//         enable_voice_detection: true,
//         enable_residual_echo_detector: false,
//         enable_level_estimation: false,
//       },

//       // for keyboard
//       enable_transient_suppression: true,
//       high_pass_filter: Some(webrtc_audio_processing::HighPassFilter {
//         apply_in_full_band: true,
//       }),

//       // pre_amplifier: Some(webrtc_audio_processing::PreAmplifier {
//       //   ..Default::default()
//       // }),
//       noise_suppression: Some(NoiseSuppression {
//         level: noise_cancel_level, // low was lower than libwebrtc
//         analyze_linear_aec_output: false,
//       }),

//       gain_controller: Some(GainController {
//         mode: GainControllerMode::AdaptiveDigital,
//         target_level_dbfs: 7, // was 3
//         compression_gain_db: 12,
//         enable_limiter: true,
//         ..Default::default()
//       }),
//       pipeline: Pipeline {
//         maximum_internal_processing_rate:
//           webrtc_audio_processing::PipelineProcessingRate::Max48000Hz,
//         multi_channel_capture: true,
//         multi_channel_render: true,
//       },

//       ..webrtc_audio_processing::Config::default()
//     };

//     processor.set_config(config);
//   }

//   pub fn num_samples_per_frame(&self) -> usize {
//     self.inner.num_samples_per_frame()
//   }

//   pub fn set_playback_delay_ms(&self, playback_delay_ms: usize) {
//     let now = Instant::now();
//     let mut pbd = self.playback_delay.lock().expect("get playback delay");
//     *pbd = (playback_delay_ms, now);
//   }

//   /// Attempt to calculate fresh delay if new estimate is available for renderer size
//   /// otherwise return None and the processor will use its last used value
//   pub fn update_current_stream_delay(&mut self, capture_delay_ms: usize) {
//     // Sets the delay in ms between process_render_frame() receiving a far-end frame
//     // and process_capture_frame() receiving a near-end frame containing the corresponding echo.

//     let stream_delay_ms = {
//       // get
//       let val = { self.playback_delay.lock().expect("to get pbd").clone() };

//       // if 0 return  None

//       let playback_delay_ms = val.0;
//       //+ Instant::now().saturating_duration_since(val.1).as_millis() as usize;
//       let ms = (playback_delay_ms + capture_delay_ms) as i32;

//       Some(ms)
//     };

//     // Avoid any unneccessary clone in the audio loop
//     // #[cfg(debug_assertions)]
//     // {
//     //   let stream_delay_ms_ = stream_delay_ms.clone();
//     //   let debugger = self.debugger.clone();
//     //   tokio::spawn(async move {
//     //     debugger.stat(StatKind::StreamDelayMs, stream_delay_ms_.into(), None);
//     //   });
//     // }

//     if let Some(stream_delay_ms) = stream_delay_ms {
//       self.inner.set_stream_delay_ms(stream_delay_ms as usize);
//     }
//   }

//   pub fn set_output_will_be_muted(&self, muted: bool) {
//     self.inner.set_output_will_be_muted(muted);
//   }

//   pub fn set_stream_key_pressed(&self, key_pressed: bool) {
//     self.inner.set_stream_key_pressed(key_pressed);
//   }

//   pub fn get_stats(&self) -> webrtc_audio_processing::Stats {
//     self.inner.get_stats()
//   }

//   pub fn process_capture_frame(
//     &mut self,
//     frame: &mut [f32],
//     caputre_delay: usize,
//   ) -> Result<(), Error> {
//     // should call conditionally?
//     self.update_current_stream_delay(caputre_delay);

//     self.set_stream_key_pressed(platform_utils::key_pressed());
//     self.inner.process_capture_frame(frame)
//   }

//   pub fn process_render_frame(&mut self, frame: &mut [f32]) {
//     self
//       .inner
//       .process_render_frame(frame)
//       .expect("to process render frame");
//   }
// }

// pub fn create_processor(
//   num_capture_channels: i32,
//   num_render_channels: i32,
//   _noise_level: NoiseSuppressionLevel,
// ) -> Result<Processor, webrtc_audio_processing::Error> {
//   let processor = Processor::new(&InitializationConfig {
//     sample_rate_hz: 48_000,
//     num_capture_channels: num_capture_channels as usize,
//     num_render_channels: num_render_channels as usize,
//   })?;

//   Ok(processor)
// }
