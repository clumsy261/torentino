#[derive(Debug, Clone)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request(u32, u32, u32),
    Piece(u32, u32, Vec<u8>),
    Cancel(u32, u32, u32),
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have,
    Bitfield,
    Request,
    Piece,
    Cancel,
}

impl MessageId {
    pub fn from_u8(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Choke),
            1 => Some(Self::Unchoke),
            2 => Some(Self::Interested),
            3 => Some(Self::NotInterested),
            4 => Some(Self::Have),
            5 => Some(Self::Bitfield),
            6 => Some(Self::Request),
            7 => Some(Self::Piece),
            8 => Some(Self::Cancel),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Choke => 0,
            Self::Unchoke => 1,
            Self::Interested => 2,
            Self::NotInterested => 3,
            Self::Have => 4,
            Self::Bitfield => 5,
            Self::Request => 6,
            Self::Piece => 7,
            Self::Cancel => 8,
        }
    }
}

pub fn encode_message(msg: &PeerMessage) -> Vec<u8> {
    match msg {
        PeerMessage::KeepAlive => vec![0, 0, 0, 0],
        PeerMessage::Choke => vec![0, 0, 0, 1, 0],
        PeerMessage::Unchoke => vec![0, 0, 0, 1, 1],
        PeerMessage::Interested => vec![0, 0, 0, 1, 2],
        PeerMessage::NotInterested => vec![0, 0, 0, 1, 3],
        PeerMessage::Have(idx) => {
            let mut buf = vec![0, 0, 0, 5, 4];
            buf.extend_from_slice(&idx.to_be_bytes());
            buf
        }
        PeerMessage::Bitfield(bits) => {
            let len = 1 + bits.len() as u32;
            let mut buf = vec![];
            buf.extend_from_slice(&len.to_be_bytes());
            buf.push(5);
            buf.extend_from_slice(bits);
            buf
        }
        PeerMessage::Request(idx, begin, length) => {
            let mut buf = vec![0, 0, 0, 13, 6];
            buf.extend_from_slice(&idx.to_be_bytes());
            buf.extend_from_slice(&begin.to_be_bytes());
            buf.extend_from_slice(&length.to_be_bytes());
            buf
        }
        PeerMessage::Piece(idx, begin, data) => {
            let len = 9 + data.len() as u32;
            let mut buf = vec![];
            buf.extend_from_slice(&len.to_be_bytes());
            buf.push(7);
            buf.extend_from_slice(&idx.to_be_bytes());
            buf.extend_from_slice(&begin.to_be_bytes());
            buf.extend_from_slice(data);
            buf
        }
        PeerMessage::Cancel(idx, begin, length) => {
            let mut buf = vec![0, 0, 0, 13, 8];
            buf.extend_from_slice(&idx.to_be_bytes());
            buf.extend_from_slice(&begin.to_be_bytes());
            buf.extend_from_slice(&length.to_be_bytes());
            buf
        }
        PeerMessage::Unknown(_) => vec![0, 0, 0, 0],
    }
}

pub fn decode_message(id: u8, payload: &[u8]) -> Result<PeerMessage, String> {
    match MessageId::from_u8(id) {
        Some(MessageId::Choke) => Ok(PeerMessage::Choke),
        Some(MessageId::Unchoke) => Ok(PeerMessage::Unchoke),
        Some(MessageId::Interested) => Ok(PeerMessage::Interested),
        Some(MessageId::NotInterested) => Ok(PeerMessage::NotInterested),
        Some(MessageId::Have) => {
            if payload.len() < 4 {
                return Err("have message too short".into());
            }
            let idx = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            Ok(PeerMessage::Have(idx))
        }
        Some(MessageId::Bitfield) => Ok(PeerMessage::Bitfield(payload.to_vec())),
        Some(MessageId::Request) => {
            if payload.len() < 12 {
                return Err("request message too short".into());
            }
            let idx = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
            let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
            Ok(PeerMessage::Request(idx, begin, length))
        }
        Some(MessageId::Piece) => {
            if payload.len() < 8 {
                return Err("piece message too short".into());
            }
            let idx = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
            let data = payload[8..].to_vec();
            Ok(PeerMessage::Piece(idx, begin, data))
        }
        Some(MessageId::Cancel) => {
            if payload.len() < 12 {
                return Err("cancel message too short".into());
            }
            let idx = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
            let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
            Ok(PeerMessage::Cancel(idx, begin, length))
        }
        None => Ok(PeerMessage::Unknown(id)),
    }
}
