use cidre::cm;

pub const N_START_CODE_LENGTH: usize = 4;
pub const N_START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

/// Convert AVCC frame to elem stream frame
pub fn to_elem_stream(sample_buff: &cm::SampleBuf, format_bytes: &[u8]) -> Vec<u8> {
  // https://stackoverflow.com/questions/28396622/extracting-h264-from-cmblockbuffer
  // TODO: use MemoryPool
  // elementary_stream
  let mut out = Vec::with_capacity(5_000);

  if let Some(data_buffer) = sample_buff.data_buf() {
    let is_i_frame = sample_buff.is_key_frame();

    if is_i_frame {
      out.extend_from_slice(format_bytes);
    }

    let block_buffer_length = data_buffer.data_len();

    let buffer_data = data_buffer.as_slice().expect("data ptr");

    // Loop through all the NAL units in the block buffer
    // and write them to the elementary stream with
    // start codes instead of AVCC length headers
    let mut buffer_offset: usize = 0;
    let avcc_header_length: usize = 4;

    while buffer_offset < (block_buffer_length - avcc_header_length) {
      // Read the NAL unit length
      let nal_unit_length: u32 = u32::from_be_bytes(
        buffer_data[buffer_offset..buffer_offset + avcc_header_length]
          .try_into()
          .unwrap(),
      );

      if nal_unit_length > 0 {
        // Write start code to the elementary stream
        out.extend_from_slice(&N_START_CODE[0..N_START_CODE_LENGTH]);

        // Write the NAL unit without the AVCC length header to the elementary stream
        out.extend_from_slice(
          &buffer_data[buffer_offset + avcc_header_length
            ..buffer_offset + avcc_header_length + nal_unit_length as usize],
        );

        // Move to the next NAL unit in the block buffer
        buffer_offset += avcc_header_length + nal_unit_length as usize;
      }
    }
  }

  out
}
