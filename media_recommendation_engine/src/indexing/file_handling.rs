use std::{
    borrow::{Borrow, Cow},
    ffi::OsStr,
    io::{Read, Seek},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Context;
use sha2::Digest;
use tracing::warn;

use crate::{state::AppResult, utils::HandleErr};

pub fn scan_dir(path: &Path, recurse: bool) -> Vec<PathBuf> {
    path.read_dir().map_or(Vec::new(), |read_dir| {
        let mut out = Vec::new();

        for entry in read_dir {
            if let Some(entry) =
                entry.log_err_with_msg("Encountered IO Error while scanning directory")
            {
                let path = entry.path();
                let is_dir = path.is_dir();
                if is_dir && recurse {
                    out.extend(scan_dir(&path, true));
                } else if !is_dir {
                    out.push(path);
                }
            }
        }
        out
    })
}

/// A trait so i stay consistent with the conversions
pub trait AsDBString {
    fn as_db_string(&self) -> Cow<'_, str>;
}

impl AsDBString for Path {
    fn as_db_string(&self) -> Cow<'_, str> {
        self.to_string_lossy()
    }
}

impl AsDBString for OsStr {
    fn as_db_string(&self) -> Cow<'_, str> {
        self.to_string_lossy()
    }
}

pub trait HashFile {
    fn hash_file(&self) -> AppResult<Vec<u8>>;
}

impl HashFile for Path {
    // Hashing takes a long time in file io, so a large amount is skipped
    // This is of course only a approximation at that point. This might be removed entirely
    // if it doesn't prove useful, but will be good enough for now
    fn hash_file(&self) -> AppResult<Vec<u8>> {
        const BUFFER_SIZE: usize = 1024 * 2048; // 2 MiB
        const SKIP_AMOUNT: i64 = BUFFER_SIZE as i64 * 15; // Skip 30 Mib for every 2 MiB read

        let mut hasher = sha2::Sha256::new();
        let mut file = std::fs::File::open(self)
            .with_context(|| format!("Failed to open \"{self:?}\" for hashing"))?;
        let mut buffer = vec![0u8; BUFFER_SIZE];
        loop {
            let Ok(count) = file.read(&mut buffer) else {
                break;
            };

            if count == 0 {
                break;
            }

            hasher.update(&buffer[..count]);

            if file.seek(std::io::SeekFrom::Current(SKIP_AMOUNT)).is_err() {
                break;
            }
        }
        Ok(hasher.finalize().to_vec())
    }
}

pub trait PathExt {
    fn last_modified(&self) -> Option<u64>;
    fn file_type(&self) -> Option<FileType>;
}

pub enum FileType {
    Video,
    Audio,
    Unknown,
}

impl PathExt for Path {
    fn last_modified(&self) -> Option<u64> {
        let Ok(metadata) = self.metadata() else {
            warn!("Failed to get metadata for {self:?}");
            return None;
        };

        metadata
            .modified()
            .expect("Your System is currently not supported")
            .duration_since(SystemTime::UNIX_EPOCH)
            .log_err_with_msg("Failed to get last modified time")
            .map(|d| d.as_secs())
    }

    /// Returns the filetype classified into what is known by the system
    /// Returns None if the path has no file extension or if it isn't valid utf-8
    fn file_type(&self) -> Option<FileType> {
        match self.extension() {
            Some(ext) => match ext.as_db_string().borrow() {
                "mp4" => Some(FileType::Video),
                "mp3" => Some(FileType::Audio),
                _ => Some(FileType::Unknown),
            },
            None => None,
        }
    }
}
