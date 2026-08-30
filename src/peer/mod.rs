pub mod message;

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::peer::message::{PeerMessage, decode_message, encode_message};

pub const BLOCK_SIZE: u32 = 16384;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RECV_TIMEOUT: Duration = Duration::from_secs(120);

pub struct PeerConnection {
    stream: TcpStream,
    pub addr: std::net::SocketAddr,
}

impl PeerConnection {
    pub async fn connect(
        addr: std::net::SocketAddr,
        info_hash: &[u8; 20],
        peer_id: &[u8; 20],
    ) -> Result<Self, std::io::Error> {
        let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("connection to {addr} timed out"),
                )
            })??;

        stream.set_nodelay(true)?;

        let mut handshake = Vec::with_capacity(68);
        handshake.push(19);
        handshake.extend_from_slice(b"BitTorrent protocol");
        handshake.extend_from_slice(&[0u8; 8]);
        handshake.extend_from_slice(info_hash);
        handshake.extend_from_slice(peer_id);

        stream.write_all(&handshake).await?;

        let mut resp = [0u8; 68];
        stream.read_exact(&mut resp).await?;

        let resp_info_hash = &resp[28..48];
        if resp_info_hash != info_hash {
            return Err(std::io::Error::other("info_hash mismatch in handshake"));
        }

        log::debug!("handshake complete with {addr}");

        Ok(PeerConnection { stream, addr })
    }

    pub async fn send(&mut self, msg: &PeerMessage) -> Result<(), std::io::Error> {
        let data = encode_message(msg);
        self.stream.write_all(&data).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Option<PeerMessage>, std::io::Error> {
        let mut len_buf = [0u8; 4];
        match tokio::time::timeout(RECV_TIMEOUT, self.stream.read_exact(&mut len_buf)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "peer read timed out",
                ));
            }
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 {
            return Ok(Some(PeerMessage::KeepAlive));
        }

        let mut msg_buf = vec![0u8; len];
        tokio::time::timeout(RECV_TIMEOUT, self.stream.read_exact(&mut msg_buf))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "peer read timed out")
            })??;

        let id = msg_buf[0];
        let payload = &msg_buf[1..];
        let msg = decode_message(id, payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(msg))
    }
}
