//! TURN over TCP and TLS. RFC 8656 frames STUN and ChannelData separately.

use async_trait::async_trait;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use webrtc_util::{Conn, Result};

trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Stream for T {}

pub struct StreamConnection {
    local: SocketAddr,
    remote: SocketAddr,
    reader: Mutex<ReadHalf<Box<dyn Stream>>>,
    writer: Mutex<WriteHalf<Box<dyn Stream>>>,
    closed: CancellationToken,
}

impl StreamConnection {
    pub async fn connect(
        remote: SocketAddr,
        tls_host: Option<&str>,
    ) -> std::result::Result<Arc<Self>, String> {
        let stream = TcpStream::connect(remote)
            .await
            .map_err(|e| e.to_string())?;
        let local = stream.local_addr().map_err(|e| e.to_string())?;
        let stream: Box<dyn Stream> = if let Some(host) = tls_host {
            use tokio_rustls::rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
            let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, stream)
                .await
                .map_err(|e| e.to_string())?;
            Box::new(stream)
        } else {
            Box::new(stream)
        };
        let (reader, writer) = tokio::io::split(stream);
        Ok(Arc::new(Self {
            local,
            remote,
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            closed: CancellationToken::new(),
        }))
    }
}

#[async_trait]
impl Conn for StreamConnection {
    async fn connect(&self, _: SocketAddr) -> Result<()> {
        Ok(())
    }
    async fn recv(&self, buffer: &mut [u8]) -> Result<usize> {
        self.recv_from(buffer).await.map(|r| r.0)
    }
    async fn recv_from(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr)> {
        let mut reader = self.reader.lock().await;
        let read = async {
            let header = buffer
                .get_mut(..4)
                .ok_or(webrtc_util::Error::ErrBufferShort)?;
            reader.read_exact(header).await?;
            let length = usize::from(u16::from_be_bytes([buffer[2], buffer[3]]));
            let (header, padded) = match buffer[0] {
                0..=3 => (20, length),
                64..=127 => (4, length.next_multiple_of(4)),
                _ => {
                    return Err(webrtc_util::Error::Other(
                        "invalid TURN stream frame".into(),
                    ));
                }
            };
            let remaining = buffer
                .get_mut(4..header + padded)
                .ok_or(webrtc_util::Error::ErrBufferShort)?;
            reader.read_exact(remaining).await?;
            Ok((header + length, self.remote))
        };
        tokio::select! {
            biased;
            () = self.closed.cancelled() => Err(webrtc_util::Error::ErrUseClosedNetworkConn),
            result = read => result,
        }
    }
    async fn send(&self, data: &[u8]) -> Result<usize> {
        self.send_to(data, self.remote).await
    }
    async fn send_to(&self, data: &[u8], _: SocketAddr) -> Result<usize> {
        let mut writer = self.writer.lock().await;
        writer.write_all(data).await?;
        if data.first().is_some_and(|b| (64..=127).contains(b)) {
            let padding = data.len().next_multiple_of(4) - data.len();
            writer.write_all(&[0; 3][..padding]).await?;
        }
        Ok(data.len())
    }
    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.local)
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.remote)
    }
    async fn close(&self) -> Result<()> {
        self.closed.cancel();
        self.writer.lock().await.shutdown().await?;
        Ok(())
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(stream: tokio::io::DuplexStream) -> StreamConnection {
        let stream: Box<dyn Stream> = Box::new(stream);
        let (reader, writer) = tokio::io::split(stream);
        StreamConnection {
            local: "127.0.0.1:1000".parse().unwrap(),
            remote: "127.0.0.1:3478".parse().unwrap(),
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
            closed: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn stream_channel_data_padding_does_not_enter_the_next_message() {
        let (client, mut server) = tokio::io::duplex(1024);
        let client = connection(client);
        let channel = [64, 1, 0, 3, 1, 2, 3];
        let mut stun = [0; 20];
        stun[1] = 1;
        stun[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
        client.send(&channel).await.unwrap();
        let mut wire = [0; 8];
        server.read_exact(&mut wire).await.unwrap();
        assert_eq!(&wire[..7], &channel);
        assert_eq!(wire[7], 0);
        server.write_all(&wire).await.unwrap();
        server.write_all(&stun).await.unwrap();
        let mut buffer = [0; 100];
        assert_eq!(client.recv(&mut buffer).await.unwrap(), 7);
        assert_eq!(&buffer[..7], &channel);
        assert_eq!(client.recv(&mut buffer).await.unwrap(), 20);
        assert_eq!(&buffer[..20], &stun);
    }
}
