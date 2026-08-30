use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};

use crate::error::Error;
use crate::metainfo::Metainfo;
use crate::peer::PeerConnection;
use crate::peer::message::PeerMessage;
use crate::piece::PieceManager;
use crate::storage::DiskManager;
use crate::tracker::{self, AnnounceResponse, TrackerEvent};

const MAX_PEERS: usize = 100;
const MAX_CONCURRENT_PEERS: usize = 20;
const CLIENT_ID_PREFIX: &[u8] = b"-TO0001-";
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const RE_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "https://tracker.gbitt.info/announce",
];

pub struct Session {
    metainfo: Metainfo,
    peer_id: [u8; 20],
    piece_manager: Arc<Mutex<PieceManager>>,
    disk_manager: DiskManager,
    port: u16,
    trackers: Vec<String>,
}

pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub pieces_done: usize,
    pub pieces_total: usize,
    pub active_peers: usize,
    pub speed: u64,
}

impl Session {
    pub fn new(metainfo: Metainfo, download_dir: &Path) -> Result<Self, Error> {
        let peer_id = generate_peer_id();
        let disk_manager = DiskManager::new(&metainfo, download_dir)?;
        let piece_manager = PieceManager::new(
            metainfo.info.piece_length,
            metainfo.total_length(),
            metainfo.info.pieces.clone(),
        );
        let trackers = effective_trackers(
            metainfo.trackers(),
            std::env::var("TORRENTINO_TRACKERS").ok().as_deref(),
        );

        Ok(Session {
            metainfo,
            peer_id,
            piece_manager: Arc::new(Mutex::new(piece_manager)),
            disk_manager,
            port: 6881,
            trackers,
        })
    }

    pub async fn run(&self) -> Result<(), Error> {
        let total_pieces = self.metainfo.info.pieces.len() as u32;
        let restored = {
            let dm = self.disk_manager.clone();
            let pm = Arc::clone(&self.piece_manager);
            let mut pm = pm.lock().await;
            pm.resume_from(move |idx, len| dm.read_piece(idx, len))
        };
        log::info!("Restored {restored}/{total_pieces} verified pieces from disk");

        let announce_resp = self.announce(TrackerEvent::Started).await?;
        log::info!(
            "Tracker returned {} peers, re-announce in {}s",
            announce_resp.peers.len(),
            announce_resp.interval
        );

        let known_peers = Arc::new(Mutex::new(HashSet::<SocketAddr>::new()));
        let active_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PEERS));

        let initial_addrs: Vec<SocketAddr> =
            announce_resp.peers.into_iter().take(MAX_PEERS).collect();

        {
            let mut kp = known_peers.lock().await;
            for addr in &initial_addrs {
                kp.insert(*addr);
            }
        }

        self.spawn_peer_tasks(&initial_addrs, &semaphore, &active_count);

        let progress_pm = Arc::clone(&self.piece_manager);
        let total = self.metainfo.total_length();
        let total_pieces = self.metainfo.info.pieces.len();
        let progress_active = Arc::clone(&active_count);

        let session_metainfo = self.metainfo.clone();
        let session_trackers = self.trackers.clone();
        let session_peer_id = self.peer_id;
        let session_port = self.port;
        let session_pm = Arc::clone(&self.piece_manager);
        let session_known = Arc::clone(&known_peers);
        let session_sem = Arc::clone(&semaphore);
        let session_active = Arc::clone(&active_count);
        let session_disk = self.disk_manager.clone();

        let reannounce_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(RE_ANNOUNCE_INTERVAL).await;

                let pm = session_pm.lock().await;
                let left = session_metainfo
                    .total_length()
                    .saturating_sub(pm.downloaded);
                let is_complete = pm.is_complete();
                drop(pm);

                if is_complete {
                    break;
                }

                let event = if left == 0 {
                    TrackerEvent::Completed
                } else {
                    TrackerEvent::Started
                };

                log::info!("Re-announcing to trackers...");

                let resp = match announce_all_trackers(
                    &session_trackers,
                    &session_metainfo.info_hash,
                    &session_peer_id,
                    session_port,
                    left,
                    event,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!("Re-announce failed: {e}");
                        continue;
                    }
                };

                {
                    let mut kp = session_known.lock().await;
                    let mut new_addrs = Vec::new();
                    for addr in resp.peers {
                        if kp.insert(addr) {
                            new_addrs.push(addr);
                        }
                    }
                    drop(kp);

                    if !new_addrs.is_empty() {
                        log::info!(
                            "Re-announce found {} new peers ({} total known)",
                            new_addrs.len(),
                            session_known.lock().await.len(),
                        );
                        new_addrs.truncate(MAX_PEERS);
                        for addr in &new_addrs {
                            let pm = Arc::clone(&session_pm);
                            let info_hash = session_metainfo.info_hash;
                            let peer_id = session_peer_id;
                            let dm = session_disk.clone();
                            let sem = Arc::clone(&session_sem);
                            let active = Arc::clone(&session_active);
                            let addr = *addr;

                            let handle = tokio::spawn(async move {
                                let _permit = sem.acquire().await.unwrap();
                                active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                match PeerConnection::connect(addr, &info_hash, &peer_id).await {
                                    Ok(conn) => {
                                        log::info!("connected to {addr} (re-announce)");
                                        let result = peer_task(conn, pm, dm).await;
                                        active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                                        if let Err(e) = result {
                                            log::debug!("{addr}: {e}");
                                        }
                                    }
                                    Err(e) => {
                                        active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                                        log::debug!("{addr}: connect failed: {e}");
                                    }
                                }
                            });
                            drop(handle);
                        }
                    }
                }
            }
        });

        let progress_handle = tokio::spawn(async move {
            let mut last_downloaded = 0u64;
            let mut last_time = std::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let pm = progress_pm.lock().await;
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(last_time).as_secs_f64();
                let speed = if elapsed > 0.0 {
                    ((pm.downloaded.saturating_sub(last_downloaded)) as f64 / elapsed) as u64
                } else {
                    0
                };
                last_downloaded = pm.downloaded;
                last_time = now;

                let ap = progress_active.load(std::sync::atomic::Ordering::Relaxed);
                let progress = Progress {
                    downloaded: pm.downloaded,
                    total,
                    pieces_done: pm.completed_pieces(),
                    pieces_total: total_pieces,
                    active_peers: ap,
                    speed,
                };
                print_progress(&progress);

                if pm.is_complete() {
                    println!("\nDownload complete!");
                    break;
                }
                drop(pm);
            }
        });

        let _ = tokio::join!(progress_handle, reannounce_handle);

        let _ = self.announce(TrackerEvent::Completed).await;

        Ok(())
    }

    fn spawn_peer_tasks(
        &self,
        addrs: &[SocketAddr],
        semaphore: &Arc<Semaphore>,
        active_count: &Arc<std::sync::atomic::AtomicUsize>,
    ) {
        for addr in addrs {
            let pm = Arc::clone(&self.piece_manager);
            let info_hash = self.metainfo.info_hash;
            let peer_id = self.peer_id;
            let disk_manager = self.disk_manager.clone();
            let sem = Arc::clone(semaphore);
            let active = Arc::clone(active_count);
            let addr = *addr;

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let mut last_err = String::new();
                for attempt in 0..3u32 {
                    match PeerConnection::connect(addr, &info_hash, &peer_id).await {
                        Ok(conn) => {
                            active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            log::info!("connected to {addr} (attempt {}/3)", attempt + 1);
                            let result = peer_task(conn, pm.clone(), disk_manager.clone()).await;
                            active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                            match result {
                                Ok(()) => {
                                    log::debug!("{addr}: session ended cleanly");
                                }
                                Err(e) => {
                                    log::debug!("{addr}: {e}");
                                    last_err = e.to_string();
                                }
                            }
                            break;
                        }
                        Err(e) => {
                            last_err = e.to_string();
                            if attempt + 1 < 3 {
                                tokio::time::sleep(RECONNECT_DELAY).await;
                            }
                        }
                    }
                }
                if !last_err.is_empty() {
                    log::debug!("{addr}: giving up: {last_err}");
                }
            });
            drop(handle);
        }
    }

    async fn announce(&self, event: TrackerEvent) -> Result<AnnounceResponse, Error> {
        let pm = self.piece_manager.lock().await;
        let left = self.metainfo.total_length().saturating_sub(pm.downloaded);
        drop(pm);

        announce_all_trackers(
            &self.trackers,
            &self.metainfo.info_hash,
            &self.peer_id,
            self.port,
            left,
            event,
        )
        .await
    }
}

async fn announce_all_trackers(
    trackers: &[String],
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    left: u64,
    event: TrackerEvent,
) -> Result<AnnounceResponse, Error> {
    let mut merged_peers = Vec::new();
    let mut seen = HashSet::new();
    let mut interval: Option<u32> = None;
    let mut any_success = false;
    let mut last_err = String::new();

    for tracker_url in trackers {
        match tracker::announce(
            tracker_url,
            info_hash,
            peer_id,
            port,
            0,
            0,
            left,
            event.clone(),
        )
        .await
        {
            Ok(resp) => {
                any_success = true;
                log::info!(
                    "Tracker {tracker_url}: {} peers, re-announce in {}s",
                    resp.peers.len(),
                    resp.interval
                );
                for peer in resp.peers {
                    if seen.insert(peer) {
                        merged_peers.push(peer);
                    }
                }
                if resp.interval > 0 {
                    interval = Some(match interval {
                        Some(iv) => iv.min(resp.interval),
                        None => resp.interval,
                    });
                }
            }
            Err(e) => {
                last_err = e.to_string();
                log::warn!("Tracker {tracker_url} failed: {e}");
            }
        }
    }

    if !any_success {
        return Err(Error::Tracker(format!("all trackers failed: {last_err}")));
    }

    Ok(AnnounceResponse {
        peers: merged_peers,
        interval: interval.unwrap_or(0),
    })
}

async fn peer_task(
    mut conn: PeerConnection,
    piece_manager: Arc<Mutex<PieceManager>>,
    disk_manager: DiskManager,
) -> Result<(), Error> {
    let addr = conn.addr;

    let pm = piece_manager.lock().await;
    let total_pieces = pm.total_pieces;
    let my_bitfield = build_bitfield(&pm);
    drop(pm);

    conn.send(&PeerMessage::Bitfield(my_bitfield.clone()))
        .await?;
    conn.send(&PeerMessage::Interested).await?;

    let mut peer_bitfield = vec![0u8; (total_pieces as usize).div_ceil(8)];
    let mut am_choked = true;
    let mut requests_in_flight: u32 = 0;
    let max_pipeline: u32 = 5;
    let mut got_bitfield = false;
    let mut checked_interest = false;

    loop {
        let msg = match conn.recv().await {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                log::debug!("{addr}: {e}");
                break;
            }
        };

        match &msg {
            PeerMessage::Bitfield(bits) => {
                peer_bitfield = bits.clone();
                got_bitfield = true;

                let pm = piece_manager.lock().await;
                let have_count = peer_bitfield.iter().map(|b| b.count_ones()).sum::<u32>();
                let need = pm.need_piece(&peer_bitfield);
                log::debug!("{addr}: has {have_count}/{total_pieces} pieces, useful={need}");
                if !need {
                    log::debug!("{addr}: peer has nothing we need, disconnecting");
                    break;
                }
                drop(pm);
            }
            PeerMessage::Unchoke => {
                am_choked = false;
                log::debug!("{addr}: unchoked us");
            }
            _ => {}
        }

        match msg {
            PeerMessage::Choke => {
                am_choked = true;
            }
            PeerMessage::Unchoke => {}
            PeerMessage::Bitfield(_) => {}
            PeerMessage::Have(idx) => {
                let byte = (idx / 8) as usize;
                if byte < peer_bitfield.len() {
                    peer_bitfield[byte] |= 1 << (7 - idx % 8);
                }

                if !checked_interest && got_bitfield {
                    checked_interest = true;
                    let pm = piece_manager.lock().await;
                    if !pm.need_piece(&peer_bitfield) {
                        drop(pm);
                        log::debug!("{addr}: peer has nothing we need, disconnecting");
                        break;
                    }
                    drop(pm);
                }
            }
            PeerMessage::Piece(idx, begin, data) => {
                requests_in_flight = requests_in_flight.saturating_sub(1);
                log::debug!(
                    "{addr}: block piece={idx} begin={begin} +{}B (inflight={requests_in_flight})",
                    data.len()
                );
                let mut pm = piece_manager.lock().await;
                if let Some(piece_data) = pm.block_received(idx, begin, &data) {
                    if pm.verify_piece(idx, &piece_data) {
                        let piece_len = pm.piece_length(idx);
                        drop(pm);
                        match disk_manager.write_piece(idx, &piece_data) {
                            Ok(()) => {
                                let mut pm = piece_manager.lock().await;
                                pm.mark_complete(idx, piece_len);
                                log::info!("piece {idx} verified, written to disk");
                            }
                            Err(e) => {
                                log::warn!("{addr}: piece {idx} write failed: {e}");
                                let mut pm = piece_manager.lock().await;
                                pm.discard_piece(idx);
                            }
                        }
                    } else {
                        log::warn!("piece {idx} hash mismatch, discarding");
                        pm.discard_piece(idx);
                    }
                }
            }
            _ => {}
        }

        while requests_in_flight < max_pipeline && !am_choked {
            let mut pm = piece_manager.lock().await;
            if pm.is_complete() {
                drop(pm);
                break;
            }
            if let Some(req) = pm.next_request(0, &peer_bitfield) {
                drop(pm);
                conn.send(&PeerMessage::Request(
                    req.piece_index,
                    req.begin,
                    req.length,
                ))
                .await?;
                requests_in_flight += 1;
                log::debug!(
                    "{addr}: req piece={} begin={} len={} (inflight={requests_in_flight})",
                    req.piece_index,
                    req.begin,
                    req.length
                );
            } else {
                drop(pm);
                log::debug!(
                    "{addr}: no request available yet (choked={am_choked}, need={})",
                    piece_manager.lock().await.need_piece(&peer_bitfield)
                );
                break;
            }
        }

        let pm = piece_manager.lock().await;
        if pm.is_complete() {
            drop(pm);
            break;
        }
        drop(pm);
    }

    Ok(())
}

fn build_bitfield(pm: &PieceManager) -> Vec<u8> {
    let len = (pm.total_pieces as usize).div_ceil(8);
    let mut bits = vec![0u8; len];
    for i in 0..pm.total_pieces {
        if pm.has_piece(i) {
            let byte = (i / 8) as usize;
            bits[byte] |= 1 << (7 - i % 8);
        }
    }
    bits
}

fn print_progress(p: &Progress) {
    let pct = if p.total > 0 {
        p.downloaded as f64 / p.total as f64 * 100.0
    } else {
        0.0
    };
    let speed_str = format_speed(p.speed);
    let downloaded_str = format_bytes(p.downloaded);
    let total_str = format_bytes(p.total);
    eprint!(
        "\r[{pct:.1}%] {downloaded_str}/{total_str} | {speed_str} | {}/{} pieces | {} active peers ",
        p.pieces_done, p.pieces_total, p.active_peers,
    );
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.2} GiB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MiB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KiB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn generate_peer_id() -> [u8; 20] {
    let mut id = [0u8; 20];
    id[..CLIENT_ID_PREFIX.len()].copy_from_slice(CLIENT_ID_PREFIX);
    for byte in id[CLIENT_ID_PREFIX.len()..].iter_mut() {
        *byte = rand::random::<u8>().wrapping_add(b'0') % 10 + b'0';
    }
    id
}

fn effective_trackers(builtin: Vec<String>, env_override: Option<&str>) -> Vec<String> {
    if let Some(raw) = env_override
        && !raw.trim().is_empty()
    {
        let parsed = parse_tracker_list(raw);
        if !parsed.is_empty() {
            log::info!(
                "TORRENTINO_TRACKERS overrides tracker list with {} tracker(s)",
                parsed.len()
            );
            return parsed;
        }
        log::warn!(
            "TORRENTINO_TRACKERS set but has no valid http(s)/udp URLs; using torrent + defaults"
        );
    }
    let merged = merge_unique(
        builtin
            .into_iter()
            .chain(DEFAULT_TRACKERS.iter().map(|s| s.to_string())),
    );
    log::info!("Using {} tracker(s): {}", merged.len(), merged.join(", "));
    merged
}

fn parse_tracker_list(raw: &str) -> Vec<String> {
    merge_unique(
        raw.split(',')
            .map(str::trim)
            .filter(|u| {
                u.starts_with("http://") || u.starts_with("https://") || u.starts_with("udp://")
            })
            .map(str::to_string),
    )
}

fn merge_unique(list: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in list {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin() -> Vec<String> {
        vec!["http://torrent.ubuntu.com:6969/announce".to_string()]
    }

    #[test]
    fn env_override_replaces_everything() {
        let t = effective_trackers(
            builtin(),
            Some("udp://tracker.opentrackr.org:1337/announce, https://tracker.gbitt.info/announce"),
        );
        assert_eq!(
            t,
            vec![
                "udp://tracker.opentrackr.org:1337/announce",
                "https://tracker.gbitt.info/announce",
            ]
        );
    }

    #[test]
    fn no_env_appends_defaults_after_builtin() {
        let t = effective_trackers(builtin(), None);
        assert_eq!(
            t.first().map(String::as_str),
            Some("http://torrent.ubuntu.com:6969/announce")
        );
        assert!(
            t.iter()
                .any(|s| s == "udp://tracker.opentrackr.org:1337/announce")
        );
        assert!(t.iter().any(|s| s == "https://tracker.gbitt.info/announce"));
    }

    #[test]
    fn invalid_env_falls_back_to_defaults() {
        let t = effective_trackers(builtin(), Some("ftp://bad, not a url"));
        assert_eq!(
            t.first().map(String::as_str),
            Some("http://torrent.ubuntu.com:6969/announce")
        );
    }

    #[test]
    fn empty_env_counts_as_unset() {
        let t = effective_trackers(builtin(), Some("  "));
        assert_eq!(
            t.first().map(String::as_str),
            Some("http://torrent.ubuntu.com:6969/announce")
        );
    }

    #[test]
    fn defaults_deduped_against_builtin() {
        let mut t = builtin();
        t.push("udp://tracker.opentrackr.org:1337/announce".to_string());
        let t = effective_trackers(t, None);
        assert_eq!(
            t.iter()
                .filter(|s| *s == "udp://tracker.opentrackr.org:1337/announce")
                .count(),
            1
        );
    }
}
