use std::collections::BTreeMap;

use crate::bencode::BencodeValue;
use crate::error::Error;
use crate::tracker::AnnounceResponse;

const MAX_NUM_WANT: u32 = 50;

#[allow(clippy::too_many_arguments)]
pub async fn announce(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: crate::tracker::TrackerEvent,
) -> Result<AnnounceResponse, Error> {
    let info_hash_encoded = urlencoded_bytes(info_hash);
    let peer_id_encoded = urlencoded_bytes(peer_id);

    let url = format!(
        "{tracker_url}?info_hash={info_hash_encoded}&peer_id={peer_id_encoded}&port={port}&\
         uploaded={uploaded}&downloaded={downloaded}&left={left}&compact=1&no_peer_id=1&\
         numwant={numwant}&event={event}",
        numwant = MAX_NUM_WANT,
        event = event.as_str(),
    );

    log::debug!("HTTP tracker announce: {url}");

    let body = http_get(&url)
        .await
        .map_err(|e| Error::Tracker(format!("HTTP request failed: {e}")))?;

    let (top, _) = crate::bencode::decode(&body)
        .map_err(|e| Error::Tracker(format!("failed to decode tracker response: {e}")))?;

    let dict = top
        .as_dict()
        .ok_or_else(|| Error::Tracker("tracker response not a dict".into()))?;

    if let Some(failure) = dict.get(b"failure reason" as &[u8]) {
        return Err(Error::Tracker(format!(
            "tracker error: {}",
            failure.as_str().unwrap_or("<non-string failure reason>")
        )));
    }

    let interval = dict
        .get(b"interval" as &[u8])
        .and_then(|v| v.as_int())
        .unwrap_or(300) as u32;

    let peers = parse_peers(dict)?;

    Ok(AnnounceResponse { peers, interval })
}

fn parse_peers(dict: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<Vec<std::net::SocketAddr>, Error> {
    if let Some(peers_val) = dict.get(b"peers" as &[u8]) {
        if let Some(peers_bytes) = peers_val.as_bytes() {
            return parse_compact_ipv4(peers_bytes);
        }
        if let Some(peers_list) = peers_val.as_list() {
            return parse_dict_peers(peers_list);
        }
    }
    Err(Error::Tracker("no peers in response".into()))
}

fn parse_compact_ipv4(data: &[u8]) -> Result<Vec<std::net::SocketAddr>, Error> {
    if !data.len().is_multiple_of(6) {
        return Err(Error::Tracker("compact peer data not multiple of 6".into()));
    }
    Ok(data
        .chunks_exact(6)
        .map(|c| {
            let ip = std::net::Ipv4Addr::new(c[0], c[1], c[2], c[3]);
            let port = u16::from_be_bytes([c[4], c[5]]);
            std::net::SocketAddr::new(ip.into(), port)
        })
        .collect())
}

fn parse_dict_peers(peers_list: &[BencodeValue]) -> Result<Vec<std::net::SocketAddr>, Error> {
    let mut peers = Vec::new();
    for peer_val in peers_list {
        if let Some(d) = peer_val.as_dict()
            && let Some(ip) = d.get(b"ip" as &[u8]).and_then(|v| v.as_str())
            && let Some(port) = d.get(b"port" as &[u8]).and_then(|v| v.as_int())
            && let Ok(addr) = format!("{ip}:{port}").parse()
        {
            peers.push(addr);
        }
    }
    Ok(peers)
}

fn urlencoded_bytes(data: &[u8]) -> String {
    data.iter().map(|b| format!("%{b:02X}")).collect()
}

const MAX_REDIRECTS: usize = 5;

struct HttpResponse {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
}

async fn http_get(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut current: url::Url = url::Url::parse(url)?;

    for _ in 0..=MAX_REDIRECTS {
        let resp = request_once(&current).await?;

        if (200..300).contains(&resp.status) {
            return Ok(resp.body);
        }

        if (300..400).contains(&resp.status)
            && let Some(location) = resp.location
        {
            log::debug!("HTTP redirect {} -> {}", resp.status, location);
            current = current.join(&location)?;
            continue;
        }

        return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
            "HTTP status {}",
            resp.status
        )));
    }

    Err(Box::<dyn std::error::Error + Send + Sync>::from(
        "too many HTTP redirects",
    ))
}

async fn request_once(
    url: &url::Url,
) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    let host = url.host_str().ok_or("no host in URL")?.to_string();
    let port = url.port_or_known_default().ok_or("unknown port")?;
    let path = if let Some(q) = url.query() {
        format!("{}?{q}", url.path())
    } else {
        url.path().to_string()
    };

    let addr = format!("{host}:{port}");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");

    let stream = TcpStream::connect(&addr).await?;
    let timeout = Duration::from_secs(30);

    if url.scheme() == "https" {
        let mut root_store = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = root_store.add(cert);
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(root_store)
        .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = rustls::pki_types::ServerName::try_from(host.clone())?;
        let tls = connector.connect(server_name, stream).await?;
        tokio::time::timeout(timeout, read_response(tls, &request))
            .await
            .map_err(|_| {
                Box::<dyn std::error::Error + Send + Sync>::from("HTTP request timed out")
            })?
    } else if url.scheme() == "http" {
        tokio::time::timeout(timeout, read_response(stream, &request))
            .await
            .map_err(|_| {
                Box::<dyn std::error::Error + Send + Sync>::from("HTTP request timed out")
            })?
    } else {
        Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
            "unsupported scheme: {}",
            url.scheme()
        )))
    }
}

async fn read_response<S>(
    mut io: S,
    request: &str,
) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    io.write_all(request.as_bytes()).await?;

    let mut buf = Vec::new();
    io.read_to_end(&mut buf).await?;

    let body_pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
        .ok_or("malformed HTTP response: missing header terminator")?;

    let head = String::from_utf8_lossy(&buf[..body_pos]);
    let body = buf[body_pos..].to_vec();

    let mut lines = head.split('\n');
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);

    let mut location = None;
    for line in lines {
        let line = line.trim_end_matches('\r');
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("location")
        {
            location = Some(value.trim().to_string());
        }
    }

    Ok(HttpResponse {
        status,
        location,
        body,
    })
}
