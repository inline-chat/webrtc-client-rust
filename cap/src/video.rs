use bytes::Bytes;
use cidre::{arc, cm, os, vt::EncodeInfoFlags};
use std::ffi::c_void;

use crate::{
  capturer::CapturerOutput,
  h264::{self, N_START_CODE},
};

pub struct RecordContext {
  pub frames_count: usize,
  pub format_desc: Option<arc::R<cm::VideoFormatDesc>>,
  pub sender: flume::Sender<CapturerOutput>,

  pub cached_format_bytes: Vec<u8>,
  pub video_start_time: Option<cm::Time>,
}

impl RecordContext {
  pub fn new(sender: flume::Sender<CapturerOutput>) -> Self {
    Self {
      frames_count: 0,
      format_desc: None,
      sender,
      video_start_time: None,
      cached_format_bytes: Vec::with_capacity(50),
    }
  }

  pub fn handle_sample_buffer(&mut self, buffer: &cm::SampleBuf) {
    // Calc seconds
    let pts = buffer.pts();
    let seconds: f64;
    if let Some(ref start) = self.video_start_time {
      let diff = pts.sub(*start);
      seconds = diff.as_secs();
    } else {
      self.video_start_time = Some(pts);
      seconds = 0.0;
    };

    // let is_depended_on_by_others = is_depended_on_by_others(buffer);
    // println!("is_depended_on_by_others {:?}", is_depended_on_by_others);

    let desc_changed;

    // Cache format description
    let desc = buffer.format_desc().expect("to have desc");
    match self.format_desc {
      None => {
        self.format_desc = Some(desc.retained());
        desc_changed = true;
      }
      Some(ref prev_desc) => {
        if desc.equal(prev_desc.as_type_ref()) {
          desc_changed = false;
        } else {
          self.format_desc = Some(desc.retained());
          desc_changed = true;
        }
      }
    }

    // Cache start bytes and format
    if desc_changed {
      self.cached_format_bytes.clear();
      let (num_params, _) = desc
        .h264_params_count_and_header_len()
        .expect("to get params count");
      //write each param-set to elementary stream
      for i in 0..num_params {
        let param = desc.h264_param_set_at(i).expect("to get param");
        self.cached_format_bytes.extend_from_slice(&N_START_CODE);
        self.cached_format_bytes.extend_from_slice(param);
      }
    }

    let out = h264::to_elem_stream(buffer, &self.cached_format_bytes);

    // Prevent sending empty frame that crashes decoder
    if !out.is_empty() {
      let _ = self.sender.try_send(CapturerOutput {
        data: Bytes::from(out),
        seconds,
      });
    }

    self.frames_count += 1;
  }
}

pub extern "C" fn callback(
  ctx: *mut RecordContext,
  _: *mut c_void,
  status: os::Status,
  flags: EncodeInfoFlags,
  buffer: Option<&cm::SampleBuf>,
) {
  if status.is_err() || buffer.is_none() {
    println!("status {:?} Flags: {:#b}", status, flags);
    return;
  }

  unsafe {
    let ctx = ctx.as_mut().unwrap_unchecked();
    let buffer = buffer.unwrap_unchecked();
    ctx.handle_sample_buffer(buffer);
  }
}
