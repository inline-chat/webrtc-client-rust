use crate::rtc::peer::channel::Sdp;
use serde::{Deserialize, Serialize};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::Candidate;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateJsonStr {
  pub candidate: String,
  pub username_fragment: Option<String>,
  pub sdp_mid: Option<String>,
  pub sdp_m_line_index: Option<usize>,
}

pub fn parse_candidate(raw: &str) -> Candidate {
  // let v = serde_json::to_string(raw).expect("to json for debug");
  // let raw = v.as_str();
  debug!("candidate raw {}", raw);
  // it's json
  if !raw.contains('{') {
    unimplemented!("cannot parse candidate that is not JSON");
  };

  // is json string
  let raw = if raw.starts_with('\"') {
    serde_json::from_str::<String>(raw).expect("to parse candidate as string first")
  } else {
    raw.to_string()
  };

  let candidate: Candidate = serde_json::from_str(raw.as_str()).expect("to have valid candidate");

  debug!(
    "candidate parsed. \nog: {} \nparsed: {}",
    raw,
    &candidate.to_string()
  );
  candidate
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SdpType {
  #[serde(rename = "offer")]
  Offer,
  #[serde(rename = "answer")]
  Answer,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SdpJson {
  #[serde(rename = "type")]
  pub sdp_type: SdpType,
  #[serde(rename = "sdp")]
  pub sdp_string: String,
}

// remote_descr.type = "offer";
// remote_descr.sdp  = msg
pub fn parse_sdp(raw: &str) -> Sdp {
  let parsed = serde_json::from_str::<SdpJson>(raw).expect("to parse json sdp");

  info!("parsing remote sdp: {}", parsed.sdp_string);
  match parsed.sdp_type {
    SdpType::Answer => Sdp::Answer(
      SdpAnswer::from_sdp_string(parsed.sdp_string.as_str()).expect("to get sdp answer"),
    ),
    SdpType::Offer => {
      Sdp::Offer(SdpOffer::from_sdp_string(parsed.sdp_string.as_str()).expect("to get sdp offer"))
    }
  }
}

// pub fn create_addr(_network: NetworkType, ip: IpAddr, port: u16) -> SocketAddr {
//   /*if network.is_tcp(){
//       return &net.TCPAddr{IP: ip, Port: port}
//   default:
//       return &net.UDPAddr{IP: ip, Port: port}
//   }*/
//   SocketAddr::new(ip, port)
// }

// /// Initiates a stun requests to `server_addr` using conn, reads the response and returns the
// /// `XORMappedAddress` returned by the stun server.
// /// Adapted from stun v0.2.
// pub async fn get_xormapped_addr(
//   conn: &Arc<dyn Conn + Send + Sync>,
//   server_addr: SocketAddr,
//   deadline: Duration,
// ) -> Result<XorMappedAddress> {
//   let resp = stun_request(conn, server_addr, deadline).await?;
//   let mut addr = XorMappedAddress::default();
//   addr.get_from(&resp)?;
//   Ok(addr)
// }

// const MAX_MESSAGE_SIZE: usize = 1280;

// pub async fn stun_request(
//   conn: &Arc<dyn Conn + Send + Sync>,
//   server_addr: SocketAddr,
//   deadline: Duration,
// ) -> Result<Message> {
//   let mut request = Message::new();
//   request.build(&[Box::new(BINDING_REQUEST), Box::new(TransactionId::new())])?;

//   conn.send_to(&request.raw, server_addr).await?;
//   let mut bs = vec![0_u8; MAX_MESSAGE_SIZE];
//   let (n, _) = if deadline > Duration::from_secs(0) {
//     match tokio::time::timeout(deadline, conn.recv_from(&mut bs)).await {
//       Ok(result) => match result {
//         Ok((n, addr)) => (n, addr),
//         Err(err) => return Err(Error::Other(err.to_string())),
//       },
//       Err(err) => return Err(Error::Other(err.to_string())),
//     }
//   } else {
//     conn.recv_from(&mut bs).await?
//   };

//   let mut res = Message::new();
//   res.raw = bs[..n].to_vec();
//   res.decode()?;

//   Ok(res)
// }
