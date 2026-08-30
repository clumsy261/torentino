pub mod http;
pub mod udp;

use std::net::SocketAddr;

use crate::error::Error;

pub struct AnnounceResponse {
    pub peers: Vec<SocketAddr>,
    pub interval: u32,
}

#[derive(Debug, Clone)]
pub enum TrackerEvent {
    Started,
    Completed,
    Stopped,
}

impl TrackerEvent {
    pub fn as_str(&self) -> &str {
        match self {
            TrackerEvent::Started => "started",
            TrackerEvent::Completed => "completed",
            TrackerEvent::Stopped => "stopped",
        }
    }
}

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
    if tracker_url.starts_with("udp://") {
        udp::announce(
            tracker_url,
            info_hash,
            peer_id,
            port,
            uploaded,
            downloaded,
            left,
            event,
        )
        .await
    } else if tracker_url.starts_with("http://") || tracker_url.starts_with("https://") {
        http::announce(
            tracker_url,
            info_hash,
            peer_id,
            port,
            uploaded,
            downloaded,
            left,
            event,
        )
        .await
    } else {
        Err(Error::Tracker(format!(
            "unsupported tracker scheme: {tracker_url}"
        )))
    }
}
