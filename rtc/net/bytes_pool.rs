use bytes::BytesMut;

pub struct BytesPool {
  pool_size: usize,
  bytes_capacity: usize,
  reserve: Vec<BytesMut>,
}

impl BytesPool {
  pub fn new(pool_size: usize, bytes_capacity: usize) -> Self {
    Self {
      reserve: vec![BytesMut::zeroed(bytes_capacity); pool_size],
      pool_size,
      bytes_capacity,
    }
  }

  pub fn get_bytes_mut(&mut self) -> BytesMut {
    if let Some(bytes) = self.reserve.pop() {
      bytes
    } else {
      // re-allocate
      for _ in 0..=self.pool_size {
        self.reserve.push(BytesMut::zeroed(self.bytes_capacity));
      }

      BytesMut::zeroed(self.bytes_capacity)
    }
  }
}
