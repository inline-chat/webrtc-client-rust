use super::utils::UserId;
use std::collections::VecDeque;

pub struct PeerInit {
  pub user_id: UserId,
  pub initial_offerer: bool,
}

pub struct PeerQueue {
  queue: VecDeque<PeerInit>,
}

impl PeerQueue {
  pub fn init() -> Self {
    Self {
      queue: VecDeque::new(),
    }
  }

  #[allow(dead_code)]
  pub fn add(&mut self, user_id: &UserId, initial_offerer: bool) {
    self.queue.push_back(PeerInit {
      user_id: user_id.to_owned(),
      initial_offerer,
    });
  }

  /// Get next peer in queue
  #[allow(dead_code)]
  pub fn next(&mut self) -> Option<PeerInit> {
    self.queue.pop_front()
  }

  /// Get all peers
  #[allow(dead_code)]
  pub fn drain(&mut self) -> VecDeque<PeerInit> {
    self.queue.drain(..).collect::<VecDeque<PeerInit>>()
  }

  /// Reset
  pub fn clear(&mut self) {
    self.queue.clear();
  }
}
