use std::collections::{BTreeMap, HashSet};

use sha1::{Digest, Sha1};

use crate::bencode::{BencodeValue, decode};
use crate::error::Error;

#[derive(Debug, Clone)]
pub struct Metainfo {
    pub announce: String,
    pub announce_list: Vec<Vec<String>>,
    pub info: Info,
    pub info_hash: [u8; 20],
}

#[derive(Debug, Clone)]
pub struct Info {
    pub piece_length: u64,
    pub pieces: Vec<[u8; 20]>,
    pub name: String,
    pub length: Option<u64>,
    pub files: Option<Vec<FileEntry>>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub length: u64,
    pub path: Vec<String>,
}

impl Metainfo {
    pub fn from_bytes(data: &[u8]) -> Result<Self, Error> {
        let (top, _) = decode(data).map_err(|e| Error::Metainfo(e.to_string()))?;
        let dict = top
            .as_dict()
            .ok_or_else(|| Error::Metainfo("top level not a dict".into()))?;

        let announce = dict
            .get(b"announce" as &[u8])
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Metainfo("missing 'announce'".into()))?
            .to_string();

        let announce_list = dict
            .get(b"announce-list" as &[u8])
            .and_then(|v| v.as_list())
            .map(|tiers| {
                tiers
                    .iter()
                    .filter_map(|tier| {
                        tier.as_list().map(|list| {
                            list.iter()
                                .filter_map(|u| u.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                    })
                    .filter(|tier| !tier.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![vec![announce.clone()]]);

        let info_val = dict
            .get(b"info" as &[u8])
            .ok_or_else(|| Error::Metainfo("missing 'info'".into()))?;
        let info_dict = info_val
            .as_dict()
            .ok_or_else(|| Error::Metainfo("'info' is not a dict".into()))?;

        let info_bytes = crate::bencode::encode(info_val);
        let mut hasher = Sha1::new();
        hasher.update(&info_bytes);
        let info_hash: [u8; 20] = hasher.finalize().into();

        let info = parse_info(info_dict)?;

        Ok(Metainfo {
            announce,
            announce_list,
            info,
            info_hash,
        })
    }

    pub fn total_length(&self) -> u64 {
        if let Some(len) = self.info.length {
            len
        } else {
            self.info
                .files
                .as_ref()
                .map(|files| files.iter().map(|f| f.length).sum())
                .unwrap_or(0)
        }
    }

    pub fn trackers(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.announce_list
            .iter()
            .flatten()
            .filter(|url| seen.insert(url.as_str()))
            .cloned()
            .collect()
    }
}

fn parse_info(dict: &BTreeMap<Vec<u8>, BencodeValue>) -> Result<Info, Error> {
    let piece_length =
        dict.get(b"piece length" as &[u8])
            .and_then(|v| v.as_int())
            .ok_or_else(|| Error::Metainfo("missing 'piece length'".into()))? as u64;

    let pieces_bytes = dict
        .get(b"pieces" as &[u8])
        .and_then(|v| v.as_bytes())
        .ok_or_else(|| Error::Metainfo("missing 'pieces'".into()))?;

    if pieces_bytes.len() % 20 != 0 {
        return Err(Error::Metainfo("pieces length not multiple of 20".into()));
    }
    let pieces: Vec<[u8; 20]> = pieces_bytes
        .chunks_exact(20)
        .map(|c| c.try_into().unwrap())
        .collect();

    let name = dict
        .get(b"name" as &[u8])
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Metainfo("missing 'name'".into()))?
        .to_string();

    let length = dict
        .get(b"length" as &[u8])
        .and_then(|v| v.as_int())
        .map(|l| l as u64);

    let files = dict
        .get(b"files" as &[u8])
        .and_then(|v| v.as_list())
        .map(|fl| {
            fl.iter()
                .filter_map(|f| {
                    let d = f.as_dict()?;
                    let len = d.get(b"length" as &[u8])?.as_int()? as u64;
                    let path_list = d.get(b"path" as &[u8])?.as_list()?;
                    let path: Vec<String> = path_list
                        .iter()
                        .filter_map(|p| {
                            p.as_bytes()
                                .and_then(|b| std::str::from_utf8(b).ok())
                                .map(String::from)
                        })
                        .collect();
                    Some(FileEntry { length: len, path })
                })
                .collect::<Vec<_>>()
        });

    Ok(Info {
        piece_length,
        pieces,
        name,
        length,
        files,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::bencode::{BencodeValue, encode};

    use super::Metainfo;

    fn info_dict() -> BencodeValue {
        let mut info = BTreeMap::new();
        info.insert(b"piece length".to_vec(), BencodeValue::Int(4));
        info.insert(b"name".to_vec(), BencodeValue::Bytes(b"x.bin".to_vec()));
        info.insert(b"length".to_vec(), BencodeValue::Int(4));
        info.insert(b"pieces".to_vec(), BencodeValue::Bytes(vec![0u8; 60]));
        BencodeValue::Dict(info)
    }

    fn make(announce: &str, announce_list: Option<Vec<Vec<&str>>>) -> Metainfo {
        let mut top = BTreeMap::new();
        top.insert(
            b"announce".to_vec(),
            BencodeValue::Bytes(announce.as_bytes().to_vec()),
        );
        if let Some(tiers) = announce_list {
            top.insert(
                b"announce-list".to_vec(),
                BencodeValue::List(
                    tiers
                        .into_iter()
                        .map(|tier| {
                            BencodeValue::List(
                                tier.into_iter()
                                    .map(|u| BencodeValue::Bytes(u.as_bytes().to_vec()))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            );
        }
        top.insert(b"info".to_vec(), info_dict());
        Metainfo::from_bytes(&encode(&BencodeValue::Dict(top))).unwrap()
    }

    #[test]
    fn trackers_flatten_tiers_and_dedupe() {
        let m = make(
            "http://a.example/announce",
            Some(vec![
                vec![
                    "https://t1.example/announce",
                    "udp://t2.example:6969/announce",
                ],
                vec!["http://a.example/announce"],
                vec!["https://t1.example/announce"],
            ]),
        );
        assert_eq!(
            m.trackers(),
            vec![
                "https://t1.example/announce",
                "udp://t2.example:6969/announce",
                "http://a.example/announce",
            ]
        );
    }

    #[test]
    fn trackers_fallback_to_primary() {
        let m = make("http://primary.example/announce", None);
        assert_eq!(m.trackers(), vec!["http://primary.example/announce"]);
        assert_eq!(
            m.announce_list,
            vec![vec!["http://primary.example/announce".to_string()]]
        );
    }
}
