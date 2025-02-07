use std::ffi::c_void;

use cidre::arc::Release;
use cidre::arc::R;
pub use cidre::cf;
use cidre::cf::Allocator;
pub use cidre::cm;
use cidre::cm::BlockBuf;
use cidre::cm::MemPool;
use cidre::cm::SampleBuf;
use cidre::cm::VideoFormatDesc;
use cidre::cv;
use cidre::cv::PixelFormat;
use cidre::os;
use cidre::vt;
pub use cidre::vt::decompression::properties::keys;
pub use cidre::vt::decompression::properties::video_decoder_specification;
pub use cidre::vt::decompression::property_keys;
pub use cidre::vt::decompression::session::Session;
use cidre::vt::DecodeInfoFlags;

pub struct Decoder {
  memory_pool: R<MemPool>,
  allocator: R<Allocator>,
  format_desc: Option<R<VideoFormatDesc>>,
  session: Option<R<Session>>,
  sender: flume::Sender<DecoderOutput>,

  sps_size: usize,
  pps_size: usize,
}

extern "C" {
  fn CMSampleBufferGetSampleAttachmentsArray(
    sbuf: &SampleBuf,
    create_if_necessary: bool,
  ) -> Option<&mut cf::ArrayOf<cf::DictionaryMut>>;
}

impl Decoder {
  pub fn new(sender: flume::Sender<DecoderOutput>) -> Self {
    let memory_pool = MemPool::new();
    let allocator = memory_pool.allocator().expect("to have allocator");
    let allocator = allocator.retained();
    Self {
      memory_pool,
      allocator,
      format_desc: None,
      session: None,
      sender,

      sps_size: 0,
      pps_size: 0,
    }
  }

  /// Decode a frame
  pub fn decode(&mut self, frame: &[u8]) {
    // todo: process frame properly

    // Ref
    // https://stackoverflow.com/questions/29525000/how-to-use-videotoolbox-to-decompress-h-264-video-stream
    // todo: what is frame size
    let frame_size = frame.len();

    let mut data: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;
    let mut sps: Option<Vec<u8>> = None;

    let start_code_index = 0;
    let mut second_start_code_index = 0;
    let mut third_start_code_index = 0;

    let mut block_length = 0;

    let mut sample_buffer: Option<R<cm::SampleBuf>> = None;
    let mut block_buffer: Option<R<cm::BlockBuf>> = None;

    let mut nalu_type = frame[start_code_index + 4] & 0x1F;

    println!("~~~~~~~ Received NALU Type \"{:?}\" ~~~~~~~~", nalu_type);

    if nalu_type != 7 && self.format_desc.is_none() {
      println!("Video error: Frame is not an I Frame and format description is null");
      return;
    }

    if nalu_type == 7 {
      for i in (start_code_index + 4)..(start_code_index + 40) {
        if frame[i] == 0x00 && frame[i + 1] == 0x00 && frame[i + 2] == 0x00 && frame[i + 3] == 0x01
        {
          second_start_code_index = i;
          self.sps_size = second_start_code_index;
          break;
        }
      }

      nalu_type = frame[second_start_code_index + 4] & 0x1F;
      println!("~~~~~~~ Received NALU Type \"{:?}\" ~~~~~~~~", nalu_type);
    }

    if nalu_type == 8 {
      for i in (self.sps_size + 4)..(self.sps_size + 30) {
        if frame[i] == 0x00 && frame[i + 1] == 0x00 && frame[i + 2] == 0x00 && frame[i + 3] == 0x01
        {
          third_start_code_index = i;
          self.pps_size = third_start_code_index - self.sps_size;
          break;
        }
      }

      sps = Some(vec![0; self.sps_size - 4]); //  - 4
      pps = Some(vec![0; self.pps_size - 4]);

      sps
        .as_mut()
        .unwrap()
        .copy_from_slice(&frame[4..self.sps_size]); //  - 4

      let pps_start = self.sps_size + 4;
      let pps_end = pps_start + self.pps_size - 4;
      pps
        .as_mut()
        .unwrap()
        .copy_from_slice(&frame[pps_start..pps_end]);

      let parameter_set_pointers = [
        sps.as_mut().unwrap().as_ptr(), // as_mut_ptr
        pps.as_mut().unwrap().as_ptr(), // as_mut_ptr
      ];
      let parameter_set_sizes = [self.sps_size - 4, self.pps_size - 4];

      let new_format_desc =
        VideoFormatDesc::with_h264_param_sets(&parameter_set_pointers, &parameter_set_sizes, 4)
          .expect("to have desc");

      let mut format_same = false;

      if self.format_desc.is_some() {
        format_same = self.format_desc.as_ref().unwrap().equal(&new_format_desc);
        // Release the existing format description
        // Save format desc
        self.format_desc = Some(new_format_desc);

        if !format_same {
          println!("creating new session");
          self.create_session();
        }
      } else {
        self.format_desc = Some(new_format_desc);
        self.create_session();
      }

      nalu_type = frame[third_start_code_index + 4] & 0x1F;
      println!("~~~~~~~ Received NALU Type \"{:?}\" ~~~~~~~~", nalu_type);
    }

    // status == noErr &&
    if self.session.is_none() {
      self.create_session();
    }

    if nalu_type == 5 {
      let offset = self.sps_size + self.pps_size;
      block_length = frame_size - offset;
      data = Some(vec![0; block_length]);
      data
        .as_mut()
        .unwrap()
        .copy_from_slice(&frame[offset..offset + block_length]);

      let data_length32 = (block_length as i32 - 4_i32).to_be();
      data.as_mut().unwrap()[0..4].copy_from_slice(&data_length32.to_ne_bytes());

      // Create block buffer
      let mut block = BlockBuf::with_mem_block(block_length, Some(&self.allocator))
        .expect("to create block buff");
      block
        .as_mut_slice()
        .unwrap()
        .copy_from_slice(data.as_ref().unwrap());
      block_buffer = Some(block);
      // block_buffer = Some(unsafe {
      //   BlockBuf::create_with_memory_block_in(
      //     data.as_mut().unwrap().as_mut_ptr() as *mut c_void,
      //     block_length,
      //     None,
      //     0,
      //     block_length,
      //     cm::BlockBufFlags::NONE,
      //     Some(&self.allocator),
      //   )
      //   .expect("to create block buff")
      // });

      // status = CMBlockBufferCreateWithMemoryBlock(
      //   data.as_mut().unwrap().as_mut_ptr(),
      //   block_length,
      //   kCFAllocatorNull,
      //   NULL,
      //   0,
      //   block_length,
      //   0,
      //   &mut block_buffer,
      // );

      // println!(
      //   "\t\t BlockBufferCreation: \t {}",
      //   if status == kCMBlockBufferNoErr {
      //     "successful!"
      //   } else {
      //     "failed..."
      //   }
      // );
    }

    if nalu_type == 1 {
      block_length = frame_size;
      data = Some(vec![0; block_length]);
      data
        .as_mut()
        .unwrap()
        .copy_from_slice(&frame[0..block_length]);

      let data_length32 = (block_length as i32 - 4_i32).to_be();
      data.as_mut().unwrap()[0..4].copy_from_slice(&data_length32.to_ne_bytes());

      // Create block buffer
      let mut block = BlockBuf::with_mem_block(block_length, Some(&self.allocator))
        .expect("to create block buff");
      block
        .as_mut_slice()
        .unwrap()
        .copy_from_slice(data.as_ref().unwrap());
      block_buffer = Some(block);
      // block_buffer = Some(unsafe {
      //   BlockBuf::create_with_memory_block_in(
      //     data.as_mut().unwrap().as_mut_ptr() as *mut c_void,
      //     block_length,
      //     None,
      //     0,
      //     block_length,
      //     cm::BlockBufFlags::NONE,
      //     Some(&self.allocator),
      //   )
      //   .expect("to create block buff")
      // });

      // println!(
      //   "\t\t BlockBufferCreation: \t {}",
      //   if status == kCMBlockBufferNoErr {
      //     "successful!"
      //   } else {
      //     "failed..."
      //   }
      // );
    }

    let sample_size = block_length;

    let mut sample_buffer = unsafe {
      cm::SampleBuf::create_in(
        Some(&self.allocator),
        Some(block_buffer.as_ref().unwrap()),
        true,
        None,
        std::ptr::null(),
        Some(self.format_desc.as_ref().unwrap()),
        1,
        0,
        std::ptr::null(),
        1,
        &sample_size,
        &mut sample_buffer,
      )
      .to_result_unchecked(sample_buffer)
    }
    .expect("to have sample buffer");

    let attachments =
      unsafe { CMSampleBufferGetSampleAttachmentsArray(&mut sample_buffer, true) }.unwrap();

    let dict = &mut attachments[0];
    dict.insert(
      cm::sample_buffer::attachment_keys::display_immediately(),
      cf::Boolean::value_true(),
    );

    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    // -----------------------
    let Some(ref session) = self.session else {
      return;
    };

    block_buffer.as_ref().unwrap().retained();
    sample_buffer.retained();
    session
      .decode(&sample_buffer, vt::DecodeFrameFlags::_1X_REAL_TIME_PLAYBACK)
      .expect("to decode");
  }

  fn create_session(&mut self) {
    let Some(format_desc) = self.format_desc.as_ref() else {
      eprintln!("format desc is none in create session");
      return;
    };
    // Encoder specifications
    let mut specs = cf::DictionaryMut::with_capacity(10);

    // Enable low latency mode
    // ref: https://developer.apple.com/videos/play/wwdc2021/10158/#
    specs.insert(
      video_decoder_specification::enable_hardware_accelerated_video_decoder(),
      cf::Boolean::value_true(),
    );

    let ctx = OutputContext {
      sender: self.sender.clone(),
    };
    let record = vt::DecompressionOutputCbRecord::new(ctx, callback);

    let mut destination_attributes = cf::DictionaryMut::with_capacity(2);
    destination_attributes.insert(
      cv::pixel_buffer_keys::pixel_format(),
      PixelFormat::_32_ARGB.to_cf_number().as_type_ref(),
    );

    let session = Session::new(
      format_desc,
      Some(&specs),
      Some(&destination_attributes),
      Some(&record),
    )
    .expect("to create session");

    self.session = session.into();
  }
}

impl Drop for Decoder {
  fn drop(&mut self) {
    self.memory_pool.invalidate();
    unsafe { self.allocator.release() };
    if let Some(mut s) = self.session.take() {
      s.invalidate()
    }
  }
}

// ---------------------
// Output callback
// ---------------------
extern "C" fn callback(
  ctx: *mut OutputContext,
  _: *mut c_void,
  status: os::Status,
  flags: DecodeInfoFlags,
  buffer: Option<&cv::ImageBuf>,
  _pts: cm::Time,
  _duration: cm::Time,
) {
  if status.is_err() || buffer.is_none() {
    println!("status {:?} Flags: {:#b}", status, flags);
    return;
  }
  unsafe {
    let ctx = ctx.as_mut().unwrap_unchecked();
    ctx.handle_image_buf(buffer.unwrap_unchecked());
  }
}

// -----------------------
// Output Context
// -----------------------

pub struct DecoderOutput {
  pub image_buf: R<cv::ImageBuf>,
}

unsafe impl Send for DecoderOutput {}
unsafe impl Sync for DecoderOutput {}
unsafe impl Send for Decoder {}

pub struct OutputContext {
  pub sender: flume::Sender<DecoderOutput>,
}

impl OutputContext {
  pub fn new(sender: flume::Sender<DecoderOutput>) -> Self {
    Self { sender }
  }

  pub fn handle_image_buf(&mut self, buffer: &cv::ImageBuf) {
    let _ = self.sender.try_send(DecoderOutput {
      image_buf: buffer.retained(),
    });
  }
}
