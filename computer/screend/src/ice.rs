//! Candidate gathering and the network I/O that str0m leaves to its caller.

use async_trait::async_trait;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use str0m::{Candidate, net::Protocol};
use tokio::net::UdpSocket;
use tokio::sync::mpsc as queue;
use turn::client::{Client, ClientConfig};
use webrtc_util::{Conn, Result as IoResult};

#[derive(Clone, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ServerTransport {
    Udp,
    Tcp,
    Tls,
}

struct ServerUrl {
    host: String,
    port: u16,
    transport: ServerTransport,
    turn: bool,
}

impl ServerUrl {
    fn parse(raw: &str) -> Result<Self, String> {
        let (scheme, rest) = raw.split_once(':').ok_or("an ICE URL needs a scheme")?;
        let secure = matches!(scheme, "turns" | "stuns");
        if !matches!(scheme, "stun" | "stuns" | "turn" | "turns") {
            return Err("unsupported ICE URL scheme".into());
        }
        let parsed = url::Url::parse(&format!("{scheme}://{rest}")).map_err(|e| e.to_string())?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err("an ICE URL contains credentials or a fragment".into());
        }
        let mut transport = if secure {
            ServerTransport::Tls
        } else {
            ServerTransport::Udp
        };
        for (key, value) in parsed.query_pairs() {
            transport = match (key.as_ref(), value.as_ref(), secure) {
                ("transport", "udp", false) => ServerTransport::Udp,
                ("transport", "tcp", false) => ServerTransport::Tcp,
                ("transport", "tcp", true) => ServerTransport::Tls,
                _ => return Err("unsupported ICE transport".into()),
            };
        }
        let host = parsed
            .host_str()
            .ok_or("an ICE URL needs a host")?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        Ok(Self {
            host,
            port: parsed.port().unwrap_or(if secure { 5349 } else { 3478 }),
            transport,
            turn: matches!(scheme, "turn" | "turns"),
        })
    }
}

pub struct Packet {
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub data: Vec<u8>,
}

struct Outbound {
    source: SocketAddr,
    destination: SocketAddr,
    data: Vec<u8>,
}

/// Dropping the sender closes the allocation and stops its refresh timers.
pub struct Transport {
    pub base: SocketAddr,
    pub relayed: Option<SocketAddr>,
    send: queue::UnboundedSender<Outbound>,
    task: tokio::task::JoinHandle<()>,
}

impl Transport {
    pub async fn close(self) {
        drop(self.send);
        let _ = self.task.await;
    }

    pub fn send(&self, source: SocketAddr, destination: SocketAddr, data: &[u8]) {
        let _ = self.send.send(Outbound {
            source,
            destination,
            data: data.to_vec(),
        });
    }
}

/// The TURN client reads its server's packets. Every other packet goes
/// to the ICE agent with the local address that received it.
struct Connection {
    socket: UdpSocket,
    server: SocketAddr,
    incoming: mpsc::Sender<Packet>,
}

#[async_trait]
impl Conn for Connection {
    async fn connect(&self, addr: SocketAddr) -> IoResult<()> {
        Ok(self.socket.connect(addr).await?)
    }
    async fn recv(&self, buf: &mut [u8]) -> IoResult<usize> {
        self.recv_from(buf).await.map(|v| v.0)
    }
    async fn recv_from(&self, buf: &mut [u8]) -> IoResult<(usize, SocketAddr)> {
        loop {
            let (n, source) = self.socket.recv_from(buf).await?;
            if source == self.server {
                return Ok((n, source));
            }
            let _ = self.incoming.send(Packet {
                source,
                destination: self.socket.local_addr()?,
                data: buf[..n].to_vec(),
            });
        }
    }
    async fn send(&self, buf: &[u8]) -> IoResult<usize> {
        Ok(self.socket.send(buf).await?)
    }
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> IoResult<usize> {
        Ok(self.socket.send_to(buf, target).await?)
    }
    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.server)
    }
    async fn close(&self) -> IoResult<()> {
        Ok(())
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

pub async fn gather(
    servers: &[IceServer],
    incoming: mpsc::Sender<Packet>,
) -> Result<(Vec<Candidate>, Vec<Transport>), String> {
    let mut candidates = Vec::new();
    let mut transports = Vec::new();
    for server in servers {
        for url in &server.urls {
            let endpoint = ServerUrl::parse(url)?;
            let remote = tokio::net::lookup_host((endpoint.host.as_str(), endpoint.port))
                .await
                .map_err(|e| e.to_string())?
                .next()
                .ok_or("the ICE server has no address")?;
            let udp = endpoint.transport == ServerTransport::Udp;
            let conn: Arc<dyn Conn + Send + Sync> = if udp {
                // A connected probe sends no packet. It selects the route's interface.
                let probe = UdpSocket::bind(if remote.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                })
                .await
                .map_err(|e| e.to_string())?;
                probe.connect(remote).await.map_err(|e| e.to_string())?;
                let ip = probe.local_addr().map_err(|e| e.to_string())?.ip();
                let socket = UdpSocket::bind((ip, 0)).await.map_err(|e| e.to_string())?;
                Arc::new(Connection {
                    socket,
                    server: remote,
                    incoming: incoming.clone(),
                })
            } else {
                let tls =
                    (endpoint.transport == ServerTransport::Tls).then_some(endpoint.host.as_str());
                tokio::time::timeout(
                    Duration::from_secs(3),
                    crate::ice_stream::StreamConnection::connect(remote, tls),
                )
                .await
                .map_err(|_| "the TURN connection timed out")??
            };
            let base = conn.local_addr().map_err(|e| e.to_string())?;
            let client = Client::new(ClientConfig {
                stun_serv_addr: remote.to_string(),
                turn_serv_addr: remote.to_string(),
                username: server.username.clone(),
                password: server.credential.clone(),
                realm: String::new(),
                software: "pagis".into(),
                rto_in_ms: 200,
                conn: conn.clone(),
                vnet: None,
            })
            .await
            .map_err(|e| e.to_string())?;
            client.listen().await.map_err(|e| e.to_string())?;
            let gathered = async {
                let mapped = client
                    .send_binding_request()
                    .await
                    .map_err(|e| e.to_string())?;
                let relay: Option<Box<dyn Conn + Send + Sync>> = if endpoint.turn {
                    Some(Box::new(
                        client.allocate().await.map_err(|e| e.to_string())?,
                    ))
                } else {
                    None
                };
                Ok::<_, String>((mapped, relay))
            };
            let (mapped, relay) = match tokio::time::timeout(Duration::from_secs(3), gathered).await
            {
                Ok(Ok(result)) => result,
                result => {
                    let _ = client.close().await;
                    return Err(format!(
                        "candidate gathering failed for {url}: {}",
                        match result {
                            Ok(Err(e)) => e,
                            _ => "timed out".into(),
                        }
                    ));
                }
            };
            if udp {
                candidates.push(Candidate::host(base, Protocol::Udp).map_err(|e| e.to_string())?);
                candidates.push(
                    Candidate::server_reflexive(mapped, base, Protocol::Udp)
                        .map_err(|e| e.to_string())?,
                );
            }
            let relayed = relay
                .as_ref()
                .map(|r| r.local_addr())
                .transpose()
                .map_err(|e| e.to_string())?;
            if let Some(address) = relayed {
                // A relay candidate has its own address as its base (RFC 8445
                // section 5.1.1.2). A host base makes str0m prune its pair.
                candidates.push(
                    Candidate::relayed(address, address, Protocol::Udp)
                        .map_err(|e| e.to_string())?,
                );
            }
            let (send, mut outbound) = queue::unbounded_channel::<Outbound>();
            let incoming = incoming.clone();
            let task = tokio::spawn(async move {
                let mut buffer = [0; 2048];
                loop {
                    tokio::select! {
                        packet = outbound.recv() => {
                            let Some(packet) = packet else { break };
                            let connection: &dyn Conn = if Some(packet.source) == relayed {
                                relay.as_deref().expect("a relay source has an allocation")
                            } else { conn.as_ref() };
                            if let Err(error) = connection.send_to(&packet.data, packet.destination).await {
                                eprintln!("[screend] ICE send failed: {error}");
                            }
                        }
                        received = async {
                            match &relay {
                                Some(relay) => relay.recv_from(&mut buffer).await,
                                None => std::future::pending().await,
                            }
                        } => {
                            let Ok((n, source)) = received else { break };
                            let _ = incoming.send(Packet { source, destination: relayed.unwrap(), data: buffer[..n].to_vec() });
                        }
                    }
                }
                if let Some(relay) = relay {
                    let _ = relay.close().await;
                }
                let _ = client.close().await;
                let _ = conn.close().await;
            });
            transports.push(Transport {
                base,
                relayed,
                send,
                task,
            });
        }
    }
    Ok((candidates, transports))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_urls_name_udp_tcp_and_tls_endpoints() {
        let udp = ServerUrl::parse("turn:relay.example.net").unwrap();
        assert_eq!(udp.port, 3478);
        assert_eq!(udp.transport, ServerTransport::Udp);
        let tcp = ServerUrl::parse("turn:relay.example.net:443?transport=tcp").unwrap();
        assert_eq!(tcp.port, 443);
        assert_eq!(tcp.transport, ServerTransport::Tcp);
        let tls = ServerUrl::parse("turns:relay.example.net").unwrap();
        assert_eq!(tls.port, 5349);
        assert_eq!(tls.transport, ServerTransport::Tls);
        assert!(ServerUrl::parse("turn:relay.example.net?transport=sctp").is_err());
    }
}
