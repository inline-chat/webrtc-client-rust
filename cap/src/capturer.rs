pub mod config;

use crate::error::Error;
use crate::utils;

use crate::audio::*;
use crate::video;
use bytes::Bytes;

use cidre::cf;

use cidre::dispatch::Queue;
use cidre::sc::Stream;
use cidre::{
  arc, at,
  cat::{AudioFormat, AudioFormatFlags},
  cm, define_obj_type, dispatch, ns, objc,
  sc::{self, stream::Output, stream::OutputImpl},
  vt::{
    self,
    compression_properties::{keys, profile_level},
  },
};
use tokio::runtime::Handle;

use self::config::CapturerConfig;

pub mod specs {
  use cidre::cf;

  #[link(name = "VideoToolbox", kind = "framework")]
  extern "C" {
    static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: &'static cf::String;
  }

  /// The number of pending frames in the compression session.
  ///
  /// This number may decrease asynchronously.
  #[doc(alias = "kVTVideoEncoderSpecification_EnableLowLatencyRateControl")]
  #[inline]
  pub fn enable_low_latency_rate_control() -> &'static cf::String {
    unsafe { kVTVideoEncoderSpecification_EnableLowLatencyRateControl }
  }
}

#[repr(C)]
struct FrameCounterInner {
  video_counter: usize,
  audio_counter: usize,
  audio_queue: AudioQueue,
  session: arc::R<vt::CompressionSession>,
  audio_converter: at::AudioConverterRef,

  keyframe_requested: bool,
}

impl FrameCounterInner {
  pub fn _video_counter(&self) -> usize {
    self.video_counter
  }

  pub fn invalidate(&mut self) {
    self.session.invalidate();
  }

  fn handle_audio(&mut self, sample_buf: &mut cm::SampleBuf) {
    // if self.audio_counter == 0 {
    //   let format_desc = sample_buf.format_desc().unwrap();
    //   let sbd = format_desc.stream_basic_desc().unwrap();
    //   println!("{:?}", sbd);
    //   self.audio_converter = configured_converter(sbd);
    // }

    // self.audio_queue.enque(sample_buf);

    // if self.audio_queue.is_ready() {
    //   let mut data = [0u8; 2000];
    //   let buffer = at::AudioBuf {
    //     number_channels: 1,
    //     data_bytes_size: data.len() as _,
    //     data: data.as_mut_ptr(),
    //   };
    //   let buffers = [buffer];
    //   let mut buf = at::audio::BufList {
    //     number_buffers: buffers.len() as _,
    //     buffers,
    //   };

    //   let mut size = 1u32;

    //   self
    //     .audio_converter
    //     .fill_complex_buf(convert_audio, &mut self.audio_queue, &mut size, &mut buf)
    //     .unwrap();

    //   // println!("size {}", buf.buffers[0].data_bytes_size,);
    // }

    // self.audio_counter += 1;
  }

  fn handle_video(&mut self, sample_buf: &mut cm::SampleBuf) {
    let Some(img) = sample_buf.image_buf() else {
      return;
    };
    self.video_counter += 1;
    let pts = sample_buf.pts();
    let dur = sample_buf.duration();

    let mut flags = None;
    let mut has_props = false;
    // let mut frame_properties = cf::DictionaryMut::with_capacity(1);
    let d = cf::DictionaryOf::with_keys_values(
      &[vt::compression_properties::frame_keys::force_key_frame()],
      &[cf::Boolean::value_true().as_type_ref()],
    );

    let frame_properties = if self.keyframe_requested {
      has_props = true;

      // add prop
      // frame_properties(
      //   vt::compression_properties::frame_keys::force_key_frame(),
      //   cf::Boolean::value_true().as_type_ref(),
      // );

      // reset for next frame
      self.keyframe_requested = false;

      Some(d.as_ref())
    } else {
      None
    };

    let res = self.session.encode_frame(
      img,
      pts,
      dur,
      if has_props { frame_properties } else { None },
      std::ptr::null_mut(),
      &mut flags,
    );

    if res.is_err() {
      println!("err {:?}", res);
    }
  }
}

define_obj_type!(FrameCounter + OutputImpl, FrameCounterInner, FRAME_COUNTER);

impl Output for FrameCounter {}

#[objc::add_methods]
impl OutputImpl for FrameCounter {
  extern "C" fn impl_stream_did_output_sample_buf(
    &mut self,
    _cmd: Option<&cidre::objc::Sel>,
    _stream: &sc::Stream,
    sample_buffer: &mut cm::SampleBuf,
    kind: sc::OutputType,
  ) {
    if kind == sc::OutputType::Screen {
      self.inner_mut().handle_video(sample_buffer)
    } else if kind == sc::OutputType::Audio {
      self.inner_mut().handle_audio(sample_buffer);
    }
  }
}

/// CAPTURE
pub struct ScreenCapturer {
  stream: Option<arc::Retained<Stream>>,
  queue: Option<arc::Retained<Queue>>,
  delegate: Option<arc::Retained<FrameCounter>>,
  // session: Option<arc::Retained<vt::compression::Session>>,

  // Something to send frames to the app
  sender: flume::Sender<CapturerOutput>,

  // Configure
  config: CapturerConfig,
  display_id: Option<u32>,

  session_props: arc::R<cf::DictionaryMut>,

  // In bytes
  current_bitrate: i32,
}
pub struct CapturerOutput {
  pub data: Bytes,
  pub seconds: f64,
}

impl ScreenCapturer {
  pub fn new(sender: flume::Sender<CapturerOutput>) -> Self {
    ScreenCapturer {
      stream: None,
      queue: None,
      delegate: None,
      // session: None,
      sender,
      config: CapturerConfig::default(),
      display_id: None,

      session_props: cf::DictionaryMut::with_capacity(10),
      current_bitrate: 0,
    }
  }

  pub fn request_keyframe(&mut self) {
    let Some(delegate) = self.delegate.as_mut() else {
      return;
    };
    let inner = delegate.inner_mut();
    inner.keyframe_requested = true;
    // session.encode_frame(image_buffer, pts, duration, frame_properties, source_frame_ref_con, info_flags_out)
  }

  pub fn config(&mut self) -> &mut CapturerConfig {
    &mut self.config
  }

  pub fn set_display(&mut self, display_id: u32) {
    self.display_id = Some(display_id);
  }

  pub async fn start(&mut self) -> anyhow::Result<i32> {
    // Is supported?
    if !utils::is_supported() {
      return Err(Error::NotSupported.into());
    }

    if !utils::has_permission() {
      return Err(Error::PermissionDenied.into());
    }

    let queue = dispatch::Queue::serial_with_ar_pool();

    // display
    let content = sc::ShareableContent::current().await.expect("content");
    let display = if let Some(id) = self.display_id {
      content.displays().iter().find(|d| d.display_id() == id)
    } else {
      content.displays().first()
    }
    .expect("had no display");

    let fps = self.config.effective_fps();
    let scale_factor = self.config.resolution_as_scale_factor();
    let width = (display.width() as f32 * scale_factor).floor() as u32;
    let height = (display.height() as f32 * scale_factor).floor() as u32;

    // filter
    let windows = ns::Array::new();
    let filter = sc::ContentFilter::with_display_excluding_windows(display, &windows);
    println!("capture frame size {width}x{height}");

    // screen
    let mut cfg = sc::StreamCfg::new();
    cfg.set_minimum_frame_interval(cm::Time::new(1, fps));
    cfg.set_width(width as usize); // * 2
    cfg.set_height(height as usize); // * 2
    cfg.set_shows_cursor(true);

    // audio
    // cfg.set_captures_audio(true);
    // cfg.set_excludes_current_process_audio(true);

    // stream
    let stream = sc::Stream::new(&filter, &cfg);

    // video compression
    let input = Box::new(video::RecordContext::new(self.sender.clone()));

    let memory_pool = cm::MemPool::new();
    let memory_pool_allocator = memory_pool.pool_allocator();

    // Encoder specifications
    let mut specs = cf::DictionaryMut::with_capacity(10);

    // Enable low latency mode
    // ref: https://developer.apple.com/videos/play/wwdc2021/10158/#
    specs.insert(
      specs::enable_low_latency_rate_control(),
      cf::Boolean::value_true(),
    );

    let mut session = vt::CompressionSession::new(
      width,
      height,
      cm::VideoCodec::H264,
      Some(&specs),
      None,
      Some(memory_pool_allocator),
      Some(video::callback),
      Box::into_raw(input) as _,
    )
    .unwrap();

    let initial_bit_rate = self.config.initial_bitrate(width, height);
    dbg!(initial_bit_rate);
    let suggested_max_qp = self.config.suggested_max_qp();
    dbg!(suggested_max_qp);

    self.current_bitrate = initial_bit_rate;

    let average_bit_rate = cf::Number::from_i32(initial_bit_rate);
    let bool_true = cf::Boolean::value_true();
    let expected_fr = cf::Number::from_i32(fps);
    let base_layer_bit_rate_fraction = cf::Number::from_f64(0.6);

    // QP is from 1-51 which lower indicates better quality at the expense of bitrate
    // and higher indicates lower quality. I chose 21 randomly.
    let max_qp = cf::Number::from_i32(suggested_max_qp);
    let max_key_frame_interval = cf::Number::from_i32(fps * 2);
    let max_key_frame_interval_duration = cf::Number::from_f64(2f64);

    let props = self.session_props.as_mut();
    props.insert(keys::real_time(), bool_true);
    // props.insert(keys::allow_frame_reordering(), bool_false);
    props.insert(keys::max_key_frame_interval(), &max_key_frame_interval);
    props.insert(
      keys::max_key_frame_interval_duration(),
      &max_key_frame_interval_duration,
    );
    props.insert(keys::avarage_bit_rate(), &average_bit_rate);
    props.insert(keys::expected_frame_rate(), &expected_fr);
    // props.insert(keys::max_frame_delay_count(), &frame_delay_count);
    props.insert(keys::max_allowed_frame_qp(), &max_qp);
    props.insert(
      keys::base_layer_bit_rate_fraction(),
      &base_layer_bit_rate_fraction,
    );
    props.insert(
      keys::profile_level(),
      profile_level::h264::constrained_high_auto_level(),
    );

    session.set_props(props).unwrap();
    session.prepare().unwrap();

    // inner core
    let input_asbd = at::audio::StreamBasicDesc {
      sample_rate: 48_000.0,
      format: AudioFormat::LINEAR_PCM,
      format_flags: AudioFormatFlags::IS_FLOAT
        | AudioFormatFlags::IS_PACKED
        | AudioFormatFlags::IS_NON_INTERLEAVED,
      bytes_per_packet: 4,
      frames_per_packet: 1,
      bytes_per_frame: 4,
      channels_per_frame: 2,
      bits_per_channel: 32,
      reserved: 0,
    };

    let inner = FrameCounterInner {
      video_counter: 0,
      audio_counter: 0,
      audio_queue: AudioQueue {
        queue: Default::default(),
        last_buffer_offset: 0,
        input_asbd,
      },
      session,
      audio_converter: default_converter(),
      keyframe_requested: false,
    };

    // delegate
    let delegate = FrameCounter::with(inner);

    stream
      .add_stream_output(delegate.as_ref(), sc::OutputType::Screen, Some(&queue))
      .unwrap();
    // TODO: audio
    // stream
    //   .add_stream_output(delegate.as_ref(), sc::OutputType::Audio, Some(&queue))
    //   .unwrap();

    if let Ok(_) = stream.start().await {
      self.queue = Some(queue);
      self.stream = Some(stream);
      self.delegate = Some(delegate);
    } else {
      // Failed to start screen
      println!("failed to start screen");
      return Err(Error::UnknownError.into());
    }

    Ok(initial_bit_rate)
  }

  /// Set bitrate for the video encoder
  pub fn set_bitrate(&mut self, bitrate: i32) {
    self.current_bitrate = bitrate - 150_000; // slightly lower
    println!("setting bitrate to {:?}", &self.current_bitrate);
    let average_bit_rate = cf::Number::from_i32(self.current_bitrate);

    self
      .delegate
      .as_mut()
      .expect("to have delegate")
      .inner_mut()
      .session
      .set_prop(keys::avarage_bit_rate(), Some(&average_bit_rate))
      .expect("to set props");
  }

  pub async fn stop(&mut self) {
    if let Some(stream) = self.stream.take() {
      let _ = stream.stop().await;
      let _ = stream.autoreleased();
    }

    if let Some(q) = self.queue.take() {
      // q.suspend();
      let _ = q.autoreleased();
    }

    if let Some(mut delegate) = self.delegate.take() {
      // Tear down the session
      delegate.inner_mut().invalidate();
    }
  }
}

impl Drop for ScreenCapturer {
  fn drop(&mut self) {
    let handle = Handle::current();
    let enter = handle.enter();
    futures::executor::block_on(self.stop());
    drop(enter);
    // self.stop().await;
  }
}
