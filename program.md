# Torentino Pure - Documentation

A pure-Rust BitTorrent client implementing the BitTorrent protocol from scratch.

## Main Executable

### `src/main.rs`

The entry point for the torrent downloading application. It sets up logging, parses command-line arguments, and runs the download session.

**Usage:**
```bash
cargo run -- <torrent-file> [download-directory]
```

**Key Components:**
- **Cli struct**: Command-line argument parser using clap. Takes a `.torrent` file path and an optional download directory.
- **DualLogger**: Custom logger that writes to both stdout and a log file (`client.log` in the download directory).
- **utc_timestamp()**: Functions to format timestamps for logging.

---

## Core Library Modules

### `src/lib.rs`

The main library entry point, exposing all public modules.

**Public Modules:**
- `bencode` - BitTorrent encoding/decoding
- `error` - Error types
- `metainfo` - Torrent file parsing
- `peer` - Peer connections
- `piece` - Piece management
- `session` - Main download session
- `storage` - Disk storage management
- `tracker` - Tracker communication

---

### `src/bencode/mod.rs`

Implements the bencode serialization format used in BitTorrent protocol.

**Key Functions:**
- `decode(input: &[u8])`: Parses bencode data from bytes into `BencodeValue` enum
- `encode(value: &BencodeValue)`: Serializes `BencodeValue` back to bencode bytes
- `decode_int`, `decode_bytes`, `decode_list`, `decode_dict`: Primitive decoders

**Used by:** Metainfo parser, tracker HTTP responses, peer messages

---

### `src/error.rs`

Defines the application's error types using `thiserror`.

**Error Variants:**
- `Bencode`: Errors during bencode parsing
- `Metainfo`: Errors parsing .torrent files
- `Tracker`: Errors communicating with trackers
- `Peer`: Errors in peer connections
- `PieceHash`: Hash verification failure for a piece
- `Io`: Standard I/O errors

---

### `src/metainfo/mod.rs`

Parses .torrent files and extracts torrent metadata.

**Key Types:**
- `Metainfo`: Represents torrent metadata including:
  - `announce`: Primary tracker URL
  - `announce_list`: Tiered list of backup trackers
  - `info`: Info dictionary containing file info
  - `info_hash`: SHA1 hash of the info dict
- `Info`: Contains `name`, `piece_length`, `pieces` (list of piece hashes), and optional `length`/`files`

**Key Functions:**
- `Metainfo::from_bytes(data)`: Parses raw .torrent file bytes
- `Metainfo::trackers()`: Returns deduplicated list of tracker URLs
- `Metainfo::total_length()`: Returns total download size

---

### `src/tracker/mod.rs`

Defines tracker communication types and routing.

**Key Types:**
- `AnnounceResponse`: Contains list of peer addresses and re-announce interval
- `TrackerEvent`: Enum for tracker events (`Started`, `Completed`, `Stopped`)

**Functions:**
- `announce()`: Routes to HTTP or UDP tracker based on URL scheme

---

### `src/tracker/http.rs`

HTTP tracker implementation.

**Key Functions:**
- `announce()`: Builds and sends HTTP announce request to tracker
- `http_get()`: Fetches HTTP response with redirect support and timeout handling
- `parse_peers()`: Parses peer list from tracker response (compact and dict formats)
- `urlencoded_bytes()`: HMAC-like encoding for info_hash and peer_id

**Features:**
- Supports HTTP/HTTPS trackers
- Follows HTTP redirects (up to 5)
- 30-second timeout per request

---

### `src/tracker/udp.rs`

UDP tracker implementation (BEP 15).

**Key Functions:**
- `announce()`: Implements UDP announce handshake
- `connect()`: UDP connection establishment to tracker
- `do_announce()`: Sends announce request and parses response

**Constants:**
- `PROTOCOL_ID`: BitTorrent UDP protocol identifier
- `CONNECT`, `ANNOUNCE`: UDP packet types
- `TIMEOUT_MS`: 15 second timeout with exponential backoff
- `MAX_RETRIES`: 8 retries

---

### `src/peer/mod.rs`

Manages peer connections for data transfer.

**Key Types:**
- `PeerConnection`: Wrapper for TCP stream with BitTorrent handshake
- `BLOCK_SIZE`: 16KB default block size for requests

**Key Functions:**
- `PeerConnection::connect()`: Performs BitTorrent handshake with peer
- `PeerConnection::send()`: Sends a peer message
- `PeerConnection::recv()`: Receives a peer message with timeout

**Handshake Protocol:**
1. Send protocol header with info_hash and peer_id
2. Receive and validate response info_hash

---

### `src/peer/message.rs`

BitTorrent peer message definitions and serialization.

**Key Types:**
- `PeerMessage`: Enum of all BitTorrent peer messages:
  - `KeepAlive`: Zero-length message
  - `Choke`, `Unchoke`: Peer control
  - `Interested`, `NotInterested`: Interest signals
  - `Have(idx)`: Announces piece availability
  - `Bitfield(bits)`: Bitmap of available pieces
  - `Request(idx, begin, length)`: Block request
  - `Piece(idx, begin, data)`: Block data
  - `Cancel(idx, begin, length)`: Cancel request

**Functions:**
- `encode_message()`: Serializes message to bytes
- `decode_message()`: Parses bytes into message

---

### `src/piece/mod.rs`

Manages piece (file block) state and requests.

**Key Types:**
- `PieceManager`: Tracks downloaded pieces, pending requests, and buffers
- `BlockRequest`: Represents a block request with piece index, offset, and length

**Key Functions:**
- `next_request()`: Selects optimal piece/block to request from available peers
- `block_received()`: Assembles blocks into complete piece data
- `verify_piece()`: SHA1 verifies piece data against expected hash
- `mark_complete()`: Marks piece as complete and updates statistics
- `resume_from()`: Restores completed pieces from disk on startup

---

### `src/session.rs`

Main download session orchestration.

**Key Types:**
- `Session`: Main torrent session holding metainfo, piece manager, disk manager
- `Progress`: Real-time download statistics for CLI display

**Key Functions:**
- `Session::new()`: Initializes session with metainfo and download directory
- `Session::run()`: Main async loop:
  1. Resumes verified pieces from disk
  2. Announces to trackers for peers
  3. Spawns peer connection tasks
  4. Manages periodic re-announcement
  5. Tracks and displays download progress
  6. Handles download completion

**Features:**
- Concurrent peer management (max 20 active peers, 100 total peers)
- Piece verification with SHA1
- Periodic tracker re-announcement (every 60s)
- Progress display with speed calculation

---

### `src/storage/mod.rs`

Disk I/O for storing downloaded data.

**Key Types:**
- `DiskManager`: Handles file creation and read/write
- `FileLayout`: Tracks file path, length, and offset in multi-file torrents

**Key Functions:**
- `DiskManager::new()`: Creates download directory structure
- `DiskManager::write_piece()`: Writes piece data to appropriate files
- `DiskManager::read_piece()`: Reads piece data for resume functionality

**Features:**
- Supports single-file and multi-file torrents
- Piece-to-file mapping for correct file placement

---

## Project Structure

```
torentino-pure/
├── Cargo.toml           # Project manifest (name: "torentino-pure")
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Library exports
│   ├── error.rs         # Error types
│   ├── bencode/         # Bencode serialization
│   ├── metainfo/        # Torrent file parsing
│   ├── tracker/         # Tracker communication (HTTP/UDP)
│   ├── peer/            # Peer connections and messages
│   ├── piece/           # Piece management
│   ├── storage/         # Disk storage
│   └── session.rs       # Session orchestration
└── target/              # Build artifacts
```

## Dependencies

- `tokio` - Async runtime
- `sha1` - SHA1 checksums for piece verification
- `bytes` - Byte buffers
- `rand` - Peer ID generation
- `thiserror` - Error derive macros
- `log` - Logging facade
- `clap` - CLI argument parsing
- `url` - URL parsing
- `rustls` / `rustls-native-certs` - TLS for HTTPS trackers
- `tokio-rustls` - Async TLS connector

## Environment Variables

- `TORRENTINO_TRACKERS` - Override tracker list (comma-separated URLs)

## Command Usage

```bash
# Download a torrent
cargo run -- /path/to/file.torrent

# Download to specific directory
cargo run -- /path/to/file.torrent /path/to/download/dir
```