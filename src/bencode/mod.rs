use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum BencodeValue {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<BencodeValue>),
    Dict(BTreeMap<Vec<u8>, BencodeValue>),
}

#[derive(Debug)]
pub struct BencodeError(pub String);

impl fmt::Display for BencodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bencode error: {}", self.0)
    }
}

impl std::error::Error for BencodeError {}

impl BencodeValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            BencodeValue::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            BencodeValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            BencodeValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[BencodeValue]> {
        match self {
            BencodeValue::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, BencodeValue>> {
        match self {
            BencodeValue::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn dict_get(&self, key: &[u8]) -> Option<&BencodeValue> {
        self.as_dict()?.get(key)
    }

    pub fn dict_get_str(&self, key: &str) -> Option<&BencodeValue> {
        self.as_dict()?.get(key.as_bytes())
    }
}

pub fn decode(input: &[u8]) -> Result<(BencodeValue, usize), BencodeError> {
    if input.is_empty() {
        return Err(BencodeError("empty input".into()));
    }
    match input[0] {
        b'i' => decode_int(input),
        b'l' => decode_list(input),
        b'd' => decode_dict(input),
        b'0'..=b'9' => decode_bytes(input),
        c => Err(BencodeError(format!("unexpected byte: {}", c))),
    }
}

fn decode_int(input: &[u8]) -> Result<(BencodeValue, usize), BencodeError> {
    let end = input
        .iter()
        .position(|&b| b == b'e')
        .ok_or_else(|| BencodeError("unterminated integer".into()))?;
    let s =
        std::str::from_utf8(&input[1..end]).map_err(|_| BencodeError("invalid integer".into()))?;
    let n: i64 = s
        .parse()
        .map_err(|_| BencodeError(format!("cannot parse int: {s}")))?;
    Ok((BencodeValue::Int(n), end + 1))
}

fn decode_bytes(input: &[u8]) -> Result<(BencodeValue, usize), BencodeError> {
    let colon = input
        .iter()
        .position(|&b| b == b':')
        .ok_or_else(|| BencodeError("missing colon in bytestring".into()))?;
    let len_str =
        std::str::from_utf8(&input[..colon]).map_err(|_| BencodeError("invalid length".into()))?;
    let len: usize = len_str
        .parse()
        .map_err(|_| BencodeError(format!("cannot parse length: {len_str}")))?;
    if colon + 1 + len > input.len() {
        return Err(BencodeError("bytestring truncated".into()));
    }
    let data = input[colon + 1..colon + 1 + len].to_vec();
    Ok((BencodeValue::Bytes(data), colon + 1 + len))
}

fn decode_list(input: &[u8]) -> Result<(BencodeValue, usize), BencodeError> {
    let mut pos = 1;
    let mut items = Vec::new();
    while pos < input.len() {
        if input[pos] == b'e' {
            return Ok((BencodeValue::List(items), pos + 1));
        }
        let (val, consumed) = decode(&input[pos..])?;
        items.push(val);
        pos += consumed;
    }
    Err(BencodeError("unterminated list".into()))
}

fn decode_dict(input: &[u8]) -> Result<(BencodeValue, usize), BencodeError> {
    let mut pos = 1;
    let mut map = BTreeMap::new();
    while pos < input.len() {
        if input[pos] == b'e' {
            return Ok((BencodeValue::Dict(map), pos + 1));
        }
        let (key, consumed) = decode(&input[pos..])?;
        pos += consumed;
        let (val, consumed) = decode(&input[pos..])?;
        pos += consumed;
        let key_bytes = match key {
            BencodeValue::Bytes(b) => b,
            _ => return Err(BencodeError("dict key must be bytestring".into())),
        };
        map.insert(key_bytes, val);
    }
    Err(BencodeError("unterminated dict".into()))
}

pub fn encode(value: &BencodeValue) -> Vec<u8> {
    match value {
        BencodeValue::Int(n) => format!("i{n}e").into_bytes(),
        BencodeValue::Bytes(b) => {
            let mut out = format!("{}:", b.len()).into_bytes();
            out.extend_from_slice(b);
            out
        }
        BencodeValue::List(items) => {
            let mut out = vec![b'l'];
            for item in items {
                out.extend_from_slice(&encode(item));
            }
            out.push(b'e');
            out
        }
        BencodeValue::Dict(map) => {
            let mut out = vec![b'd'];
            for (k, v) in map {
                out.extend_from_slice(&encode(&BencodeValue::Bytes(k.clone())));
                out.extend_from_slice(&encode(v));
            }
            out.push(b'e');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_int() {
        let v = BencodeValue::Int(42);
        let enc = encode(&v);
        assert_eq!(enc, b"i42e");
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn roundtrip_bytes() {
        let v = BencodeValue::Bytes(b"hello".to_vec());
        let enc = encode(&v);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn roundtrip_list() {
        let v = BencodeValue::List(vec![
            BencodeValue::Int(1),
            BencodeValue::Bytes(b"two".to_vec()),
        ]);
        let enc = encode(&v);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn roundtrip_dict() {
        let mut map = BTreeMap::new();
        map.insert(b"key".to_vec(), BencodeValue::Bytes(b"val".to_vec()));
        map.insert(b"num".to_vec(), BencodeValue::Int(99));
        let v = BencodeValue::Dict(map);
        let enc = encode(&v);
        let (dec, _) = decode(&enc).unwrap();
        assert_eq!(dec, v);
    }
}
