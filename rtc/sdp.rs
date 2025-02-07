use serde::{Deserialize, Serialize};

use super::peer::channel::Signal;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum SignalJson {
  Sdp { sdp: String },

  Candidate { candidate: String },
}

pub fn parse_signal_json(_signal_json: String) -> Signal {
  todo!();
}

// mod tests {

//   #[test]
//   fn test_parse_sdp() {
//     let parsed = serde_json::from_str::<SignalJson>("{\"type\":\"sdp\", \"sdp\": \"\"}").expect("");
//     assert_eq!(
//       parsed,
//       SignalJson::Sdp {
//         sdp: String::from("")
//       }
//     );
//   }
// }
