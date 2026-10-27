//! What a read can keep between runs, under the user's cache directory,
//! never in a repository or a tracker. Deleting it changes no output: every
//! entry is checked against its source before use, and a source that does
//! not match is read again from the start.
//!
//! Transcripts are append-only: a transcript's parse is kept with where it
//! stopped, and the next read resumes there.

#![allow(
    clippy::disallowed_methods,
    reason = "the cache is andon's one file it writes, in the user's cache directory"
)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::sessions::{Resume, Transcript};

/// Bumped whenever a parse would come out differently: older entries are
/// then never read.
const FORMAT: &str = "transcripts-v1";

/// The cache's directory: `$ANDON_CACHE`, else `$XDG_CACHE_HOME/andon`,
/// else `~/.cache/andon`. `ANDON_CACHE=off` keeps nothing.
#[must_use]
pub fn dir() -> Option<PathBuf> {
    match std::env::var_os("ANDON_CACHE") {
        Some(v) if v == "off" => return None,
        Some(v) if !v.is_empty() => return Some(PathBuf::from(v)),
        _ => {}
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("andon"))
}

/// Kept transcript parses.
pub struct Transcripts {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    dev: u64,
    ino: u64,
    /// A hash of the bytes just before where the read stopped: the file
    /// still holds what was read.
    tail: u64,
    resume: Resume,
    transcript: Transcript,
}

/// How many bytes before the stop are checked.
const TAIL: u64 = 256;

impl Transcripts {
    /// The cache, when there is a directory for it.
    #[must_use]
    pub fn open() -> Option<Self> {
        Some(Self {
            dir: dir()?.join(FORMAT),
        })
    }

    /// A cache in `dir`.
    #[must_use]
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: dir.join(FORMAT),
        }
    }

    fn entry_path(&self, file: &Path) -> PathBuf {
        self.dir.join(format!(
            "{:016x}.json",
            fnv(file.as_os_str().as_encoded_bytes())
        ))
    }

    /// The kept parse of `file`, if it still describes the file's start.
    #[must_use]
    pub fn get(&self, file: &Path) -> Option<(Transcript, Resume)> {
        let text = std::fs::read(self.entry_path(file)).ok()?;
        let entry: Entry = serde_json::from_slice(&text).ok()?;
        let meta = std::fs::metadata(file).ok()?;
        let same = entry.path == file
            && entry.dev == meta.dev()
            && entry.ino == meta.ino()
            && meta.len() >= entry.resume.offset
            && tail_hash(file, entry.resume.offset) == Some(entry.tail);
        same.then_some((entry.transcript, entry.resume))
    }

    /// Keep `file`'s parse up to `resume`. A cache that cannot be written
    /// is no cache: the read is the same either way.
    pub fn put(&self, file: &Path, transcript: &Transcript, resume: &Resume) {
        let Ok(meta) = std::fs::metadata(file) else {
            return;
        };
        let Some(tail) = tail_hash(file, resume.offset) else {
            return;
        };
        let entry = Entry {
            path: file.to_path_buf(),
            dev: meta.dev(),
            ino: meta.ino(),
            tail,
            resume: resume.clone(),
            transcript: transcript.clone(),
        };
        let Ok(text) = serde_json::to_vec(&entry) else {
            return;
        };
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        // written whole, then moved into place: a reader never sees half
        let path = self.entry_path(file);
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// A kept value of `kind` for `key`, if one is there and reads.
#[must_use]
pub fn load<T: serde::de::DeserializeOwned>(kind: &str, key: &str) -> Option<T> {
    let path = dir()?
        .join(kind)
        .join(format!("{:016x}.json", fnv(key.as_bytes())));
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Keep `value` of `kind` for `key`; a cache that cannot be written is no
/// cache.
pub fn store<T: Serialize>(kind: &str, key: &str, value: &T) {
    let Some(dir) = dir().map(|d| d.join(kind)) else {
        return;
    };
    let Ok(text) = serde_json::to_vec(value) else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{:016x}.json", fnv(key.as_bytes())));
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A hash of the up to `TAIL` bytes before `offset`.
fn tail_hash(file: &Path, offset: u64) -> Option<u64> {
    let start = offset.saturating_sub(TAIL);
    let mut f = File::open(file).ok()?;
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = vec![0_u8; usize::try_from(offset - start).ok()?];
    f.read_exact(&mut buf).ok()?;
    Some(fnv(&buf))
}

/// FNV-1a: stable across builds, which a `HashMap` hasher is not.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
