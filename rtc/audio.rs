use std::fmt::Display;

use cpal::{
  traits::{DeviceTrait, HostTrait},
  Device, DevicesError, SampleRate, SupportedBufferSize,
};

use crate::store::DEFAULT_DEVICE;

pub fn get_default_output_device() -> Result<Device, DevicesError> {
  #[cfg(any(
    not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
    not(feature = "jack")
  ))]
  let host = cpal::default_host();

  // Set up the input device and stream with the default input config.
  let device = host
    .default_output_device()
    .expect("failed to find output device");

  Ok(device)
}

/// used in decoder thread
pub fn get_output_device() -> cpal::StreamConfig {
  let sample_rate: u32 = 48_000;
  let channels: u16 = 2;

  let output_device = get_default_output_device().expect("to have output device");
  let mut supported_configs = output_device
    .supported_output_configs()
    .expect("to have output device");
  let default_config = output_device
    .default_output_config()
    .expect("to have default output config for sample rate");
  let supports_sample_rate = &supported_configs
    .find(|config| {
      config.max_sample_rate() >= SampleRate(sample_rate)
        && config.min_sample_rate() <= SampleRate(sample_rate)
    })
    .is_some();

  let is_sample_rate_native_default = default_config.sample_rate().0 == sample_rate;

  let sample_rate = if *supports_sample_rate {
    SampleRate(sample_rate)
  } else {
    default_config.sample_rate()
  };
  info!(
    "output device > default > buffer size {:#?}",
    default_config.buffer_size()
  );
  info!(
    "output device > default > sample rate {:#?}",
    default_config.sample_rate()
  );

  info!("Output device sample rate {}", sample_rate.0);

  // 240 * 48/44.1*2
  let _ideal_buff_size = 240; // for final output after resampling to be close to 480;
  let _ideal_buff_size = 480; // for better sync with echo cancel
                              // min or 512 or default if None
                              // *48/44.1*2 = 960 = 10ms
  let ideal_buff_size = if is_sample_rate_native_default {
    480
  } else {
    441
  };
  let min_supported_buffer_size: Option<u32> = match output_device
    .default_output_config()
    .expect("to have output config")
    .buffer_size()
  {
    SupportedBufferSize::Range { min, max } => {
      if *min <= ideal_buff_size && ideal_buff_size <= *max {
        Some(ideal_buff_size)
      } else {
        Some(min.to_owned())
      }
    }
    SupportedBufferSize::Unknown => None,
  };

  cpal::StreamConfig {
    buffer_size: if let Some(buffer_size) = min_supported_buffer_size {
      cpal::BufferSize::Fixed(buffer_size)
    } else {
      // default can be super large, so we only fallback to it if Unknown
      cpal::BufferSize::Default
    },

    channels,
    sample_rate,
  }
}

pub fn get_default_input_device() -> Result<Device, DevicesError> {
  #[cfg(any(
    not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
    not(feature = "jack")
  ))]
  let host = cpal::default_host();

  // Set up the input device and stream with the default input config.
  let device = host
    .default_input_device()
    .expect("failed to find default input device");

  Ok(device)
}

pub fn get_input_device_by_name(device_name: String) -> Result<Device, DevicesError> {
  #[cfg(any(
    not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
    not(feature = "jack")
  ))]
  let host = cpal::default_host();
  // dbg!(&device_name);

  // Set up the input device and stream with the default input config.
  let device = host
    .input_devices()
    .expect("failed to find input devices")
    .find(|device| {
      let name = device.name().unwrap_or("Audio Device".to_string());
      dbg!(&name);
      name == device_name
    });

  // If picked device not found, pick default
  if let Some(preferred) = device {
    Ok(preferred)
  } else {
    warn!(
      "Could not use preferred input device {} falling back to default",
      &device_name
    );
    get_default_input_device()
  }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
  #[error(transparent)]
  DeviceError(#[from] cpal::DevicesError),
  ConfigError(#[from] cpal::DefaultStreamConfigError),
  SupportedStreamConfigError(#[from] cpal::SupportedStreamConfigsError),
  // #[error("directory already exists")]
  // DirectoryExists,
}

use std::fmt;

impl Display for AudioError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "Audio error")
  }
}

/// used in encoder thread
pub fn get_input_device(
  preferred_sample_rate: u32,
  preferred_mic: Option<String>,
  buffer_size: u32,
) -> Result<(cpal::Device, cpal::StreamConfig), AudioError> {
  let input_device = if preferred_mic == Some(DEFAULT_DEVICE.to_string()) || preferred_mic.is_none()
  {
    get_default_input_device()?
  } else if let Some(preferred_mic) = preferred_mic {
    get_input_device_by_name(preferred_mic)?
  } else {
    unreachable!("preferred_mic should be Some or None")
  };

  let default_config = input_device.default_input_config()?;
  let mut supported_configs = input_device.supported_input_configs()?;
  let supports_track_sample_rate = &supported_configs
    .find(|config| {
      config.max_sample_rate() >= SampleRate(preferred_sample_rate)
        && config.min_sample_rate() <= SampleRate(preferred_sample_rate)
    })
    .is_some();

  // min or 512 or default if None
  let min_supported_buffer_size: Option<u32> = match input_device
    .default_input_config()
    .expect("to have input config")
    .buffer_size()
  {
    SupportedBufferSize::Range { min, max } => {
      // prev 512
      // 960 / 4 = 240 to fill a whole
      if *min <= buffer_size && buffer_size <= *max {
        Some(buffer_size)
      } else {
        Some(min.to_owned())
      }
    }
    SupportedBufferSize::Unknown => None,
  };

  let config = cpal::StreamConfig {
    // todo: must check with supported output config
    buffer_size: if let Some(buffer_size) = min_supported_buffer_size {
      cpal::BufferSize::Fixed(buffer_size)
    } else {
      // default can be super large, so we only fallback to it if Unknown
      cpal::BufferSize::Default
    },

    // Never set this to fixed 2 , it causes left channel to be filled
    channels: 1,
    // channels: default_config.channels(),
    // channels: 1,
    // sample_rate: SampleRate(24000),
    // sample_rate: SampleRate(44100),
    sample_rate: if *supports_track_sample_rate {
      SampleRate(preferred_sample_rate)
    } else {
      default_config.sample_rate()
    },
  };

  Ok((input_device, config))
}

pub fn opus_channels(channels: u16) -> opus::Channels {
  if channels == 1 {
    opus::Channels::Mono
  } else {
    opus::Channels::Stereo
  }
}

pub fn cpal_err_fn(err: cpal::StreamError) {
  eprintln!("an error occurred on stream: {}", err);
}

pub fn get_opus_samples_count(sample_rate: u32, channels: u16, duration: u32) -> usize {
  (sample_rate as usize / 1000 * duration /* ms */ as usize) * channels as usize
}
