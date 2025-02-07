use super::commands::ConnectionState;
use str0m::IceConnectionState;

/// Used in engine
pub struct PeerState {
  initial_offerer: bool,
  got_first_offer: bool,
  got_first_answer: bool,
  connection_state: ConnectionState,
  ice_connection_state: IceConnectionState,
  // whether it received one "connected" ice connection state
  established_once: bool,
}

impl PeerState {
  pub fn new(initial_offerer: bool) -> Self {
    Self {
      initial_offerer,
      got_first_offer: false,
      got_first_answer: false,
      connection_state: ConnectionState::Connecting,
      ice_connection_state: IceConnectionState::New,
      established_once: false,
    }
  }

  pub fn got_first_offer(&self) -> bool {
    self.got_first_offer
  }

  pub fn set_got_first_offer(&mut self) {
    self.got_first_offer = true;
  }

  pub fn got_first_answer(&self) -> bool {
    self.got_first_answer
  }

  pub fn set_got_first_answer(&mut self) {
    self.got_first_answer = true;
  }

  pub fn is_initial_offerer(&self) -> bool {
    self.initial_offerer
  }

  pub fn connection_state(&self) -> &ConnectionState {
    &self.connection_state
  }

  pub fn set_connection_state(&mut self, state: ConnectionState) {
    self.connection_state = state;
  }

  pub fn ice_connection_state(&self) -> &IceConnectionState {
    &self.ice_connection_state
  }

  pub fn set_ice_connection_state(&mut self, state: IceConnectionState) {
    if state == IceConnectionState::Connected {
      self.established_once = true;
    }

    self.ice_connection_state = state;
  }

  pub fn has_connected_once(&self) -> bool {
    self.established_once
  }
}
