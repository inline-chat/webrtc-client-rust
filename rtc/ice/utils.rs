use crate::rtc::{error::EngineError, net::bytes_pool::BytesPool};
use anyhow::anyhow;
use async_std::net::UdpSocket;
use bytes::Bytes;
use std::{
  net::{IpAddr, SocketAddr},
  sync::Arc,
};
use systemstat::{Platform, System};

use super::gatherer::Socket;

pub fn find_host_addresses() -> Result<Vec<IpAddr>, EngineError> {
  let system = System::new();
  let networks = system.networks()?;

  let mut hosts = Vec::with_capacity(3);
  for net in networks.values() {
    for n in &net.addrs {
      if let systemstat::IpAddr::V4(v) = n.addr {
        if !v.is_loopback() && !v.is_link_local() && !v.is_broadcast() {
          hosts.push(IpAddr::V4(v));
        }
      }
    }
  }

  info!("networks found: {:#?}", &hosts);

  if hosts.is_empty() {
    Err(EngineError::IceNoNetworkInterface)
  } else {
    Ok(hosts)
  }
}

/// Given socket, fills the buffer
pub async fn socket_recv_from(
  socket: &Socket,
  // socket: &UdpSocket,
  bytes_pool: &mut BytesPool,
  // buf: &mut Vec<u8>,
) -> Result<(SocketAddr, Bytes), EngineError> {
  // buf.resize(2000, 0);
  let mut bytes = bytes_pool.get_bytes_mut();

  match socket.recv_from(bytes.as_mut()).await {
    Ok((n, source)) => {
      // buf.truncate(n);
      // let contents = buf.as_slice().try_into().expect("failed to get buf");
      // let bytes = Bytes::copy_from_slice(contents);
      // return (source, bytes.into()).into();

      // new way
      bytes.truncate(n);
      let bytes = bytes.freeze();
      Ok((source, bytes))
    }

    Err(e) => match e {
      // taken from str0m example
      // webrtc::util::Error::ErrTimeout => ,
      // ErrorKind::WouldBlock | ErrorKind::TimedOut => None,
      _ => Err(EngineError::from(e)),
    },
  }
}

pub async fn stun_binding(socket: Arc<UdpSocket>) -> Result<SocketAddr, anyhow::Error> {
  let mut client = stun_client::Client::from_socket(socket, None);
  let res = client
    .binding_request("stun.l.google.com:19302", None)
    .await?;

  let class = res.get_class();
  match class {
    stun_client::Class::SuccessResponse => {
      let xor_mapped_addr = stun_client::Attribute::get_xor_mapped_address(&res);

      Ok(xor_mapped_addr.expect("to get ip from stun response"))
    }
    _ => Err(anyhow!(format!(
      "failed to request stun. class: {:?}",
      class
    ))),
  }
}
