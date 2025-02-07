use async_std::net::UdpSocket;
use async_trait::async_trait;
use std::net::SocketAddr;

use webrtc::util::Result;

pub struct UdpConn(pub UdpSocket);

#[async_trait]
impl webrtc::util::Conn for UdpConn {
  async fn connect(&self, addr: SocketAddr) -> Result<()> {
    Ok(self.0.connect(addr).await?)
  }

  async fn recv(&self, buf: &mut [u8]) -> Result<usize> {
    Ok(self.0.recv(buf).await?)
  }

  async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
    Ok(self.0.recv_from(buf).await?)
  }

  async fn send(&self, buf: &[u8]) -> Result<usize> {
    Ok(self.0.send(buf).await?)
  }

  async fn send_to(&self, buf: &[u8], target: SocketAddr) -> Result<usize> {
    Ok(self.0.send_to(buf, target).await?)
  }

  fn local_addr(&self) -> Result<SocketAddr> {
    Ok(self.0.local_addr()?)
  }

  fn remote_addr(&self) -> Option<SocketAddr> {
    None
  }

  async fn close(&self) -> Result<()> {
    Ok(())
  }
}
