use std::collections::VecDeque;

use cidre::cat::{AudioFormat, AudioFormatFlags};
use cidre::{arc, at, cm, os};

pub fn default_converter() -> at::AudioConverterRef {
  let output_asbd = at::audio::StreamBasicDesc {
    //sample_rate: 32_000.0,
    // sample_rate: 44_100.0,
    sample_rate: 48_000.0,
    format: AudioFormat::MPEG4_AAC,
    format_flags: Default::default(),
    // format_flags: AudioFormatFlags(MPEG4ObjectID::AAC_LC.0 as _),
    bytes_per_packet: 0,
    frames_per_packet: 1024,
    bytes_per_frame: 0,
    channels_per_frame: 2,
    bits_per_channel: 0,
    reserved: 0,
  };
  let input_asbd = at::audio::StreamBasicDesc {
    //sample_rate: 32_000.0,
    // sample_rate: 44_100.0,
    sample_rate: 48_000.0,
    format: AudioFormat::LINEAR_PCM,
    //format_flags: AudioFormatFlags(41),
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
  at::AudioConverterRef::with_formats(&input_asbd, &output_asbd).unwrap()
}

pub fn configured_converter(input_asbd: &at::audio::StreamBasicDesc) -> at::AudioConverterRef {
  // https://www.youtube.com/watch?v=yArrLvMYng8
  let output_asbd = at::audio::StreamBasicDesc {
    //sample_rate: 32_000.0,
    // sample_rate: 44_100.0,
    sample_rate: 48_000.0,
    format: AudioFormat::MPEG4_AAC_HE,
    //format_flags: AudioFormatFlags(MPEG4ObjectID::AAC_LC.0 as _),
    format_flags: AudioFormatFlags(0),
    bytes_per_packet: 0,
    frames_per_packet: 1024,
    bytes_per_frame: 0,
    channels_per_frame: 2,
    bits_per_channel: 0,
    reserved: 0,
  };

  at::AudioConverterRef::with_formats(input_asbd, &output_asbd).unwrap()
}

pub struct AudioQueue {
  pub queue: VecDeque<arc::R<cm::SampleBuf>>,
  pub last_buffer_offset: i32,
  pub input_asbd: at::audio::StreamBasicDesc,
}

impl AudioQueue {
  #[inline]
  pub fn enque(&mut self, sbuf: &cm::SampleBuf) {
    self.queue.push_back(sbuf.retained())
  }

  #[inline]
  pub fn is_ready(&self) -> bool {
    self.queue.len() > 2
  }

  pub fn fill_audio_buffer(&mut self, list: &mut at::audio::BufList<2>) -> Result<(), os::Status> {
    let mut left = 1024i32;
    let mut offset: i32 = self.last_buffer_offset;
    let mut out_offset = 0;
    let mut cursor = list.cursor();
    while let Some(b) = self.queue.pop_front() {
      let samples = b.num_samples() as i32;
      let count = i32::min(samples - offset, left);
      b.copy_pcm_data_into_audio_buf_list(
        offset,
        count,
        cursor.offset(out_offset, count as _, &self.input_asbd),
      )?;
      left -= count;
      offset += count;
      out_offset += count as usize;
      if offset < samples {
        self.last_buffer_offset = offset;
        self.queue.push_front(b);
        break;
      } else {
        offset = 0;
      }
      if left == 0 {
        break;
      }
    }
    Ok(())
  }
}

pub extern "C" fn convert_audio(
  _converter: &at::AudioConverter,
  _io_number_data_packets: &mut u32,
  io_data: &mut at::audio::BufList,
  _out_data_packet_descriptions: *mut *mut at::audio::StreamPacketDesc,
  in_user_data: *mut AudioQueue,
) -> os::Status {
  let q: &mut AudioQueue = unsafe { &mut *in_user_data };

  match q.fill_audio_buffer(unsafe { std::mem::transmute(io_data) }) {
    Ok(()) => os::Status(0),
    Err(status) => status,
  }

  //let frames = i32::min(*io_number_data_packets as i32, buf.num_samples() as _);
  //buf.copy_pcm_data_into_audio_buffer_list(0, frames, io_data)
}
