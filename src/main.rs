use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
//hey
use clap::Parser;
use log::{Level, LevelFilter, Log, Metadata, Record};

use torentino_pure::metainfo::Metainfo;
use torentino_pure::session::Session;

#[derive(Parser)]
#[command(name = "torrentino")]
#[command(about = "A pure-Rust BitTorrent client")]
struct Cli {
    /// Path to the .torrent file
    torrent: PathBuf,

    /// Download directory (defaults to current directory)
    #[arg(default_value = ".")]
    download_dir: PathBuf,
}

struct DualLogger {
    file: Option<Mutex<File>>,
}

impl Log for DualLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("[{} {}] {}", utc_timestamp(), record.level(), record.args());
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
        }
        if record.level() <= Level::Info {
            eprintln!("{line}");
        }
    }

    fn flush(&self) {
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = f.flush();
        }
    }
}

fn init_logging(path: &Path) {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(Mutex::new);

    let _ = log::set_logger(Box::leak(Box::new(DualLogger { file })));
    log::set_max_level(LevelFilter::Debug);
}

fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719468;
    let era = (z - if z >= 0 { 0 } else { 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    (if mo <= 2 { y + 1 } else { y }, mo, d)
}

fn main() {
    let cli = Cli::parse();

    let log_path = cli.download_dir.join("client.log");
    if let Err(e) = std::fs::create_dir_all(&cli.download_dir) {
        eprintln!("Failed to create download directory: {e}");
        std::process::exit(1);
    }
    init_logging(&log_path);

    let torrent_data = match std::fs::read(&cli.torrent) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read torrent file: {e}");
            std::process::exit(1);
        }
    };

    let metainfo = match Metainfo::from_bytes(&torrent_data) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to parse torrent file: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("Torrent: {}", metainfo.info.name);
    eprintln!("Tracker: {}", metainfo.announce);
    eprintln!("Size: {}", format_size(metainfo.total_length()));
    eprintln!("Pieces: {}", metainfo.info.pieces.len());
    eprintln!("Piece size: {}", format_size(metainfo.info.piece_length));

    let session = match Session::new(metainfo, &cli.download_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to create session: {e}");
            std::process::exit(1);
        }
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        if let Err(e) = session.run().await {
            eprintln!("Session error: {e}");
            std::process::exit(1);
        }
    });
}

fn format_size(bytes: u64) -> String {
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
