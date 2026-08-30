#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bencode: {0}")]
    Bencode(String),
    #[error("metainfo: {0}")]
    Metainfo(String),
    #[error("tracker: {0}")]
    Tracker(String),
    #[error("peer: {0}")]
    Peer(String),
    #[error("piece hash mismatch at piece {0}")]
    PieceHash(u32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::bencode::BencodeError> for Error {
    fn from(e: crate::bencode::BencodeError) -> Self {
        Error::Bencode(e.to_string())
    }
}
