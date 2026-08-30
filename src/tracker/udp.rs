use std::net::SocketAddr;

use tokio::net::UdpSocket;

use crate::error::Error;
use crate::tracker::{AnnounceResponse, TrackerEvent};

const PROTOCOL_ID: u64 = 0x41727101980;
const CONNECT: u32 = 0;
const ANNOUNCE: u32 = 1;
const TIMEOUT_MS: u64 = 15000;
const MAX_RETRIES: u32 = 8;
const NUM_WANT: i32 = 50;

#[allow(clippy::too_many_arguments)]
pub async fn announce(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: TrackerEvent,
) -> Result<AnnounceResponse, Error> {
    let addr = resolve_tracker_addr(tracker_url).await?;
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(&addr).await?;

    let connection_id = connect(&socket, &addr).await?;
    let resp = do_announce(
        &socket,
        connection_id,
        info_hash,
        peer_id,
        port,
        uploaded,
        downloaded,
        left,
        event,
    )
    .await?;
    Ok(resp)
}

async fn connect(socket: &UdpSocket, addr: &SocketAddr) -> Result<u64, Error> {
    for attempt in 0..MAX_RETRIES {
        let transaction_id: u32 = rand::random();
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&PROTOCOL_ID.to_be_bytes());
        buf[8..12].copy_from_slice(&CONNECT.to_be_bytes());
        buf[12..16].copy_from_slice(&transaction_id.to_be_bytes());

        socket.send(&buf).await?;

        let mut resp = [0u8; 16];
        match tokio::time::timeout(
            std::time::Duration::from_millis(TIMEOUT_MS * 2u64.pow(attempt)),
            socket.recv(&mut resp),
        )
        .await
        {
            Ok(Ok(_)) => {}
            _ => continue,
        }

        let action = u32::from_be_bytes(resp[0..4].try_into().unwrap());
        let tid = u32::from_be_bytes(resp[4..8].try_into().unwrap());
        if action != CONNECT || tid != transaction_id {
            continue;
        }
        let connection_id = u64::from_be_bytes(resp[8..16].try_into().unwrap());
        log::debug!("UDP tracker connected, id={connection_id}");
        return Ok(connection_id);
    }

    Err(Error::Tracker(format!(
        "UDP connect to {addr} failed after {MAX_RETRIES} retries"
    )))
}

#[allow(clippy::too_many_arguments)]
async fn do_announce(
    socket: &UdpSocket,
    connection_id: u64,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: TrackerEvent,
) -> Result<AnnounceResponse, Error> {
    let event_val: u32 = match event {
        TrackerEvent::Started => 2,
        TrackerEvent::Completed => 1,
        TrackerEvent::Stopped => 3,
    };

    for attempt in 0..MAX_RETRIES {
        let transaction_id: u32 = rand::random();
        let key: u32 = rand::random();
        let mut buf = [0u8; 98];
        buf[0..8].copy_from_slice(&connection_id.to_be_bytes());
        buf[8..12].copy_from_slice(&ANNOUNCE.to_be_bytes());
        buf[12..16].copy_from_slice(&transaction_id.to_be_bytes());
        buf[16..36].copy_from_slice(info_hash);
        buf[36..56].copy_from_slice(peer_id);
        buf[56..64].copy_from_slice(&downloaded.to_be_bytes());
        buf[64..72].copy_from_slice(&left.to_be_bytes());
        buf[72..80].copy_from_slice(&uploaded.to_be_bytes());
        buf[80..84].copy_from_slice(&event_val.to_be_bytes());
        buf[84..88].copy_from_slice(&0u32.to_be_bytes());
        buf[88..92].copy_from_slice(&key.to_be_bytes());
        buf[92..96].copy_from_slice(&NUM_WANT.to_be_bytes());
        buf[96..98].copy_from_slice(&port.to_be_bytes());

        socket.send(&buf).await?;

        let mut resp = vec![0u8; 1024];
        match tokio::time::timeout(
            std::time::Duration::from_millis(TIMEOUT_MS * 2u64.pow(attempt)),
            socket.recv(&mut resp),
        )
        .await
        {
            Ok(Ok(n)) => {
                resp.truncate(n);
            }
            _ => continue,
        }

        if resp.len() < 20 {
            continue;
        }

        let action = u32::from_be_bytes(resp[0..4].try_into().unwrap());
        let tid = u32::from_be_bytes(resp[4..8].try_into().unwrap());
        if action != ANNOUNCE || tid != transaction_id {
            continue;
        }

        let interval = u32::from_be_bytes(resp[8..12].try_into().unwrap());
        let peers_data = &resp[20..];
        let peers = parse_compact_peers(peers_data);

        log::debug!("UDP announce: {} peers, interval={interval}", peers.len());
        return Ok(AnnounceResponse { peers, interval });
    }

    Err(Error::Tracker("UDP announce failed after retries".into()))
}

fn parse_compact_peers(data: &[u8]) -> Vec<SocketAddr> {
    data.chunks_exact(6)
        .map(|c| {
            let ip = std::net::Ipv4Addr::new(c[0], c[1], c[2], c[3]);
            let port = u16::from_be_bytes([c[4], c[5]]);
            SocketAddr::new(ip.into(), port)
        })
        .collect()
}

async fn resolve_tracker_addr(url: &str) -> Result<SocketAddr, Error> {
    let parsed = url::Url::parse(url).map_err(|e| Error::Tracker(format!("bad URL: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::Tracker("no host in URL".into()))?
        .to_ascii_lowercase();
    let port = parsed.port().unwrap_or(80);
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return Ok(SocketAddr::new(ip.into(), port));
    }
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        return Ok(SocketAddr::new(ip.into(), port));
    }
    let first = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| Error::Tracker(format!("resolve {host}:{port}: {e}")))?;
    if let Some(addr) = first.into_iter().next() {
        return Ok(addr);
    }
    Err(Error::Tracker(format!(
        "cannot resolve tracker addr: {host}:{port}"
    )))
}
