// goals for this
// 1. gather host candidates
// 2. for each, get srflx remote ip
// 3. get turn remote allocation and relay packets
// 4. repeat

use crate::rtc::error::EngineError;
use crate::rtc::ice::utils::stun_binding;
use crate::rtc::net::bytes_pool::BytesPool;
use crate::rtc::peer2::RunLoopEvent;
use async_std::net::UdpSocket;
use flume::Receiver;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use std::sync::Arc;
use str0m::net::Transmit;
use str0m::Candidate;
use tokio::time::sleep;
use webrtc::turn::client::Client;
use webrtc::util::Conn;

use super::conn::UdpConn;
use super::utils::socket_recv_from;

pub struct IceGatherer {
  peer_event_sender: flume::Sender<RunLoopEvent>,
  /// awaiting for poll
  receiver: Receiver<Gathered>,
  sender: flume::Sender<Gathered>,
  sockets: HashMap<SocketAddr, Socket>,
  // sockets: HashMap<SocketAddr, Arc<UdpSocket>>,
  /// to close socket listeners
  socket_signals: HashMap<SocketAddr, flume::Sender<()>>,
  local_mapped: HashMap<SocketAddr, SocketAddr>,
  turn_clients: HashMap<SocketAddr, Client>,
  ice_policy: IcePolicy,
}

#[derive(Clone)]
pub(super) enum Socket {
  Conn(Arc<dyn Conn + Send + Sync>),
  Raw(Arc<UdpSocket>),
}

// Compat layer between turn and stun impl
impl Socket {
  pub fn local_addr(&self) -> Result<SocketAddr, EngineError> {
    match &self {
      Self::Conn(inner) => inner.local_addr().map_err(EngineError::from),
      Self::Raw(inner) => inner.local_addr().map_err(EngineError::from),
    }
  }

  pub async fn send_to(&self, buf: &[u8], target: SocketAddr) -> Result<usize, EngineError> {
    match &self {
      Self::Conn(inner) => inner.send_to(buf, target).await.map_err(EngineError::from),
      Self::Raw(inner) => inner.send_to(buf, target).await.map_err(EngineError::from),
    }
  }

  pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), EngineError> {
    match &self {
      Self::Conn(inner) => inner.recv_from(buf).await.map_err(EngineError::from),
      Self::Raw(inner) => inner.recv_from(buf).await.map_err(EngineError::from),
    }
  }
}

pub struct Gathered {
  pub local: Candidate,
  pub remote: Candidate,

  // pub(self) socket: Arc<UdpSocket>,
  pub(self) socket: Socket,
  pub(self) local_socket_addr: SocketAddr,
  pub(self) turn_client: Option<Client>,
}

impl Drop for IceGatherer {
  fn drop(&mut self) {
    warn!("closing ice gatherer");
    // close sockets
    for (_, signal) in self.socket_signals.iter() {
      let _ = signal.try_send(());
    }

    // close turn clients
    for (_, client) in self.turn_clients.iter() {
      let client_ = client.clone();
      tokio::spawn(async move {
        // to avoid crash for last packets
        sleep(std::time::Duration::from_secs(1)).await;
        client_.close().await.expect("to close turn");
        info!("closed turn.");
      });
    }
  }
}
#[derive(Debug, PartialEq, Clone)]
pub enum IcePolicy {
  Relay = 0,
  All = 1,
  Local = 2,
}

impl IceGatherer {
  pub fn new(peer_event_sender: flume::Sender<RunLoopEvent>, ice_policy: IcePolicy) -> Self {
    // channel to pass candidates from threads
    let (sender, receiver) = flume::bounded::<Gathered>(500);

    info!("ice transport policy {:#?}", &ice_policy);

    Self {
      receiver,
      sender,
      peer_event_sender,
      sockets: HashMap::with_capacity(10),
      socket_signals: HashMap::with_capacity(10),
      local_mapped: HashMap::with_capacity(10),
      turn_clients: HashMap::with_capacity(5),
      ice_policy,
    }
  }

  pub fn gather(&self) {
    let sender_ = self.sender.clone();
    let ice_policy = self.ice_policy.clone();

    info!("Gathering ICE candidates...");

    // Start gathering
    tokio::spawn(async move {
      // find network interfaces
      for addr in super::utils::find_host_addresses().expect("to find host interfaces") {
        if matches!(ice_policy, IcePolicy::Local | IcePolicy::All) {
          // UDP
          // add host candidates
          let addr_ = addr;
          let sender = sender_.clone();
          tokio::spawn(async move {
            if let Ok(gathered) = Self::make_host(addr_, "udp").await {
              let _ = sender.try_send(gathered);
            } else {
              warn!("failed to gather host for udp address {}", &addr_);
            }
          });

          // // TCP
          // let addr_ = addr;
          // let sender = sender_.clone();
          // tokio::spawn(async move {
          //   if let Ok(gathered) = Self::make_host(addr_, "tcp").await {
          //     let _ = sender.try_send(gathered);
          //   } else {
          //     warn!("failed to gather host for tcp address {}", &addr_);
          //   }
          // });

          // Add srflx first
          // STUN addresses
          let stun_servers = [
            "stun.l.google.com:19302".to_string(),
          ];

          for stun_server in stun_servers {
            let addr_ = addr;
            let sender = sender_.clone();
            tokio::spawn(async move {
              if let Ok(gathered) = Self::make_srflx(addr_, stun_server).await {
                let _ = sender.try_send(gathered);
              } else {
                warn!("failed to gather srflx for address {}", &addr_);
              }
            });
          }
        }

        if matches!(ice_policy, IcePolicy::Relay | IcePolicy::All) {
          // add turn
          let addr_ = addr;
          let sender = sender_.clone();
          tokio::spawn(async move {
            // add TURN with a delay
            if let Ok(gathered) = Self::make_turn(addr_).await {
              // sleep(std::time::Duration::from_millis(150)).await;
              let _ = sender.try_send(gathered);
            } else {
              warn!("failed to gather relayed for address {}", &addr_);
            }
          });
        }
      }
    });
  }

  /// Get available candidates waiting to be added to agent
  pub async fn poll_candidate(&mut self) -> Option<Gathered> {
    // listen on channel
    match self.receiver.recv_async().await {
      Ok(result) => {
        // start listening to packets
        self.start_listening(&result);

        // save ref turn client to close later
        if let Some(client) = result.turn_client.as_ref() {
          self.turn_clients.insert(
            result.socket.local_addr().expect("to get addr"),
            client.clone(),
          );
        }

        info!("Gathered candidate {:?}", &result.remote);

        result.into()
      }
      Err(_) => None,
    }
  }

  /// Send outgoing packets through the socket
  pub async fn send_to(&self, transmit: Transmit) {
    // let socket_addr = &transmit.source;
    let socket_addr = if let Some(s) = self.local_mapped.get(&transmit.source) {
      s
    } else {
      &transmit.source
    };

    if let Some(socket) = self.sockets.get(socket_addr) {
      let socket_ = socket.clone();
      tokio::spawn(async move {
        if let Err(error) = socket_
          // if let Err(error) = socket
          .send_to(&transmit.contents, transmit.destination)
          .await
        {
          debug!("failed to send {}", error);
        }
      });

      // I tried this and this caused weird delay
      // if let Err(error) = socket
      //   .send_to(&*transmit.contents, transmit.destination)
      //   .await
      // {
      //   error!("failed to send {}", error);
      // }
    } else {
      warn!("socket to transmit with not found {:#?}", transmit);
    }
  }

  // -------------------------
  // --- end of public API ---
  // -------------------------
  async fn make_host(
    addr: IpAddr,
    proto: impl TryInto<str0m::net::Protocol>,
  ) -> Result<Gathered, EngineError> {
    let socket = UdpSocket::bind(format!("{addr}:0")).await?;
    let local_addr = socket.local_addr().expect("a local socket adddress");

    let candidate = Candidate::host(local_addr, proto)?;

    Ok(Gathered {
      local: candidate.clone(),
      remote: candidate,
      socket: Socket::Raw(Arc::new(socket)),
      local_socket_addr: local_addr,
      turn_client: None,
    })
  }

  async fn make_turn(addr: IpAddr) -> Result<Gathered, EngineError> {
    use webrtc::turn::client;

    let local_socket = UdpSocket::bind(format!("{addr}:0")).await?;
    let _local_addr = local_socket.local_addr()?;

    // Your TURN server
    let turn_server_addr = "0.0.0.0:3478".to_string();

    let cfg = client::ClientConfig {
      stun_serv_addr: String::new(),
      turn_serv_addr: turn_server_addr,
      username: "your usermame".to_string(),
      password: "your password".to_string(),
      realm: "your realm".to_string(),
      software: String::new(),
      rto_in_ms: 0,
      conn: Arc::new(UdpConn(local_socket)),
      vnet: None,
    };

    let client = client::Client::new(cfg).await?;

    client.listen().await?;

    // Allocate a relay socket on the TURN server. On success, it
    // will return a net.PacketConn which represents the remote
    // socket.
    let relay_conn = client.allocate().await?;

    // Send BindingRequest to learn our external IP
    // let mapped_addr = client.send_binding_request().await?;

    let relay_addr = relay_conn.local_addr()?;
    // The relayConn's local address is actually the transport
    // address assigned on the TURN server.
    info!("relayed-address={}", &relay_addr);

    // let local = Candidate::host(local_addr.clone())?;
    // let remote = Candidate::relayed(relay_addr, local_addr, "udp".to_string())?;
    // let local = Candidate::host(local_addr.clone())?;
    let remote = Candidate::relayed(relay_addr, "udp")?;

    Ok(Gathered {
      local: remote.clone(),
      remote,
      socket: Socket::Conn(Arc::new(relay_conn)),

      // could be different
      local_socket_addr: relay_addr,
      turn_client: Some(client),
    })
  }

  async fn make_srflx(addr: IpAddr, stun_server: String) -> Result<Gathered, EngineError> {
    let local_socket = UdpSocket::bind(format!("{addr}:0")).await?;
    let local_addr = local_socket.local_addr().expect("a local socket adddress");

    debug!("STUN server connecting to: {stun_server}");
    let socket = Arc::new(local_socket);
    let public_addr = match stun_binding(socket.clone()).await {
      Ok(public_addr) => {
        info!("got srflx candidate {:#?}", &public_addr);
        public_addr
      }
      Err(error) => {
        warn!("failed to gather SRFLX candidates {:#?}", error);
        return Err(EngineError::IceStunForInterface);
      }
    };

    // SRFLX candidates are added as Host locally
    let local = Candidate::host(local_addr.clone(), str0m::net::Protocol::Udp)?;
    let remote = Candidate::server_reflexive(public_addr, local_addr, str0m::net::Protocol::Udp)?;

    Ok(Gathered {
      local,
      remote,
      socket: Socket::Raw(socket),

      // could be different
      local_socket_addr: local_addr,
      turn_client: None,
    })
  }

  /// Start listening on a socket, add it to hashmap
  fn start_listening(&mut self, gathered: &Gathered) {
    let socket_addr = gathered.socket.local_addr().expect("to get local addr");
    self.sockets.insert(socket_addr, gathered.socket.clone());

    self
      .local_mapped
      .insert(gathered.local_socket_addr, socket_addr);

    let (signal, closed) = flume::bounded::<()>(1);
    self.socket_signals.insert(socket_addr, signal);

    let socket_addr_ = socket_addr;
    let socket_ = gathered.socket.clone();
    let peer_runloop_sender = self.peer_event_sender.clone();

    // Recv packets
    tokio::spawn(async move {
      let mut bytes_pool = BytesPool::new(20, 2000);

      loop {
        tokio::select! {
          Ok((source, bytes)) = socket_recv_from(&socket_, &mut bytes_pool) => {
            let _ = peer_runloop_sender.try_send(RunLoopEvent::NetworkInput {
              bytes,
              source,
              destination: socket_addr_,
            });
          }

          _ = closed.recv_async() => {
            break;
          }
        }
      }
      debug!("socket loop task closed {}", &socket_addr_);
    });
  }
}
