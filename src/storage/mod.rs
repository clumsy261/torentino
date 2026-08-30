use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::metainfo::Metainfo;

#[derive(Clone)]
pub struct DiskManager {
    name: String,
    piece_length: u64,
    files: Vec<FileLayout>,
}

#[derive(Clone)]
struct FileLayout {
    path: PathBuf,
    length: u64,
    offset: u64,
}

impl DiskManager {
    pub fn new(metainfo: &Metainfo, download_dir: &Path) -> Result<Self, Error> {
        let name = metainfo.info.name.clone();
        let dir = download_dir.join(&name);

        match std::fs::create_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }

        let mut files = Vec::new();
        let mut offset = 0u64;

        if let Some(length) = metainfo.info.length {
            let file_path = dir.join(&name);
            if file_path.is_dir() {
                return Err(std::io::Error::other(format!(
                    "output path is a directory, not a file: {}",
                    file_path.display()
                ))
                .into());
            }
            files.push(FileLayout {
                path: file_path,
                length,
                offset,
            });
        } else if let Some(ref file_entries) = metainfo.info.files {
            for entry in file_entries {
                let mut file_path = dir.clone();
                for component in &entry.path {
                    file_path = file_path.join(component);
                }
                if let Some(parent) = file_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if file_path.is_dir() {
                    return Err(std::io::Error::other(format!(
                        "output path is a directory, not a file: {}",
                        file_path.display()
                    ))
                    .into());
                }
                files.push(FileLayout {
                    path: file_path,
                    length: entry.length,
                    offset,
                });
                offset += entry.length;
            }
        }

        Ok(DiskManager {
            name,
            piece_length: metainfo.info.piece_length,
            files,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn write_piece(&self, piece_idx: u32, data: &[u8]) -> Result<(), Error> {
        let start_offset = piece_idx as u64 * self.piece_length;
        let end_offset = start_offset + data.len() as u64;

        for file_layout in &self.files {
            let file_start = file_layout.offset;
            let file_end = file_start + file_layout.length;

            if end_offset <= file_start || start_offset >= file_end {
                continue;
            }

            let overlap_start = start_offset.max(file_start);
            let overlap_end = end_offset.min(file_end);
            let piece_slice_start = (overlap_start - start_offset) as usize;
            let piece_slice_end = (overlap_end - start_offset) as usize;
            let file_pos = overlap_start - file_start;

            use std::io::{Seek, SeekFrom, Write};
            if file_layout.path.is_dir() {
                return Err(std::io::Error::other(format!(
                    "output path is a directory, not a file: {}",
                    file_layout.path.display()
                ))
                .into());
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&file_layout.path)?;
            file.seek(SeekFrom::Start(file_pos))?;
            file.write_all(&data[piece_slice_start..piece_slice_end])?;
        }

        Ok(())
    }

    pub fn read_piece(&self, piece_idx: u32, length: u64) -> Option<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let start_offset = piece_idx as u64 * self.piece_length;
        let end_offset = start_offset + length;

        let mut data = vec![0u8; length as usize];
        let mut filled = 0usize;

        for file_layout in &self.files {
            let file_start = file_layout.offset;
            let file_end = file_start + file_layout.length;

            if end_offset <= file_start || start_offset >= file_end {
                continue;
            }

            let overlap_start = start_offset.max(file_start);
            let overlap_end = end_offset.min(file_end);
            let piece_slice_start = (overlap_start - start_offset) as usize;
            let file_pos = overlap_start - file_start;
            let span = (overlap_end - overlap_start) as usize;

            let mut file = std::fs::File::open(&file_layout.path).ok()?;
            file.seek(SeekFrom::Start(file_pos)).ok()?;
            let mut buf = vec![0u8; span];
            file.read_exact(&mut buf).ok()?;
            data[piece_slice_start..piece_slice_start + span].copy_from_slice(&buf);
            filled += span;
        }

        if filled == length as usize {
            Some(data)
        } else {
            None
        }
    }
}
