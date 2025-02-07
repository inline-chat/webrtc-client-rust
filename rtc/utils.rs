use std::fmt;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct UserId(pub String);

impl fmt::Display for UserId {
  #[inline]
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&*self.0, f)
  }
}

impl From<String> for UserId {
  fn from(user_id: String) -> Self {
    UserId(user_id)
  }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct CallId(pub String);
impl fmt::Display for CallId {
  #[inline]
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&*self.0, f)
  }
}
