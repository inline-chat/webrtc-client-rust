use super::{
  peer::channel::{Sdp, Signal},
  utils::UserId,
};
use std::collections::{HashMap, VecDeque};

pub struct PendingSignals {
  signals: HashMap<UserId, VecDeque<Signal>>,
}

impl PendingSignals {
  pub fn init() -> Self {
    Self {
      signals: HashMap::new(),
    }
  }

  pub fn add(&mut self, user_id: &UserId, signal: Signal) {
    if let Some(user_signals) = self.signals.get_mut(user_id) {
      // only add if candidate
      let should_add = match signal {
        Signal::Candidate(_) => true,
        _ => false,
      };

      if should_add {
        user_signals.push_back(signal);
      } else {
        warn!(
          "did not add signal to pending queue despite peer not being present 1 {:?}",
          signal
        );
      }
    } else {
      // only add if sdp
      let should_add = match signal {
        Signal::Sdp(_) => true,
        _ => false,
      };

      if should_add {
        self
          .signals
          .insert(user_id.to_owned(), VecDeque::from([signal]));
      } else {
        warn!(
          "did not add signal to pending queue despite peer not being present 2 {:?}",
          signal
        );
      }
    }
  }

  fn should_return(&self, user_id: &UserId, initial_offerer: bool, is_drain_all: bool) -> bool {
    if let Some(user_signals) = self.signals.get(user_id) {
      let signal = user_signals.front();

      // check first signle
      match signal {
        // only return if it's useful and doesn't cause error
        Some(Signal::Sdp(ref sdp)) => match &sdp {
          Sdp::Offer(_) => !initial_offerer,

          // DO NOT SET ANSWER WHEN WE JUST SENT OFFER
          Sdp::Answer(_) => false,
          // RTCSdpType::Answer => initial_offerer,
        },
        // candidates are safe because we've already checked at the front of deque
        Some(Signal::Candidate(_)) => {
          // only allow if we're not draining all otherwise this is the first one and we should discard the whole thing
          !is_drain_all
        }
        _ => false,
      }
    } else {
      false
    }
  }

  /// Check if should return and return all signals at once
  pub fn drain(&mut self, user_id: &UserId, initial_offerer: bool) -> Option<VecDeque<Signal>> {
    let should_return = self.should_return(user_id, initial_offerer, true);
    if self.signals.get_mut(user_id).is_some() {
      // it's not in a good shape, clean it and return None
      if !should_return {
        self.remove(user_id);
        None
      } else {
        self.remove(user_id)
        // Some(user_signals.to_owned())
      }
    } else {
      self.remove(user_id);
      None
    }
  }

  /// Get all offers for user and removes all (no checks)
  pub fn remove(&mut self, user_id: &UserId) -> Option<VecDeque<Signal>> {
    self.signals.remove(user_id)
  }

  /// Get all offers and removes all
  pub fn reset(&mut self) {
    self.signals.clear();
  }
}
