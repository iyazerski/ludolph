use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

pub(crate) fn open(path: &Path) -> Result<(BufReader<File>, u64, SystemTime)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    Ok((BufReader::new(file), meta.len(), meta.modified()?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark<'a> {
    pub id: &'a str,
    pub version: u32,
    /// The end of the last complete line read.
    pub offset: u64,
    pub len: u64,
    pub modified: SystemTime,
}

pub(crate) enum Reopen {
    Unchanged,
    Rewritten,
    Appended {
        reader: BufReader<File>,
        len: u64,
        modified: SystemTime,
    },
}

pub(crate) fn reopen(path: &Path, mark: &Mark) -> Result<Reopen> {
    let (mut reader, len, modified) = open(path)?;
    if len == mark.len && modified == mark.modified {
        return Ok(Reopen::Unchanged);
    }
    if len < mark.offset {
        return Ok(Reopen::Rewritten);
    }
    match read_header(&mut reader, &mut Vec::new(), path)? {
        Some(header) if header.id == mark.id && header.version == mark.version => {}
        _ => return Ok(Reopen::Rewritten),
    }
    reader
        .seek(SeekFrom::Start(mark.offset))
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(Reopen::Appended {
        reader,
        len,
        modified,
    })
}

/// Leave an unterminated final line for the next read; pi may still be writing it.
pub(crate) fn read_line(
    reader: &mut BufReader<File>,
    buf: &mut Vec<u8>,
    path: &Path,
) -> Result<Option<u64>> {
    buf.clear();
    let len = reader
        .read_until(b'\n', buf)
        .with_context(|| format!("reading {}", path.display()))?;
    if buf.last() != Some(&b'\n') {
        return Ok(None);
    }
    Ok(Some(
        u64::try_from(len).expect("a line is shorter than u64::MAX"),
    ))
}

/// Replace lone JSON surrogate escapes in place; JSON.stringify emits them but serde rejects them.
pub(crate) fn fix_surrogates(buf: &mut [u8]) {
    let hex = |buf: &[u8], at: usize| -> Option<u16> {
        let digits = std::str::from_utf8(buf.get(at..at + 4)?).ok()?;
        u16::from_str_radix(digits, 16).ok()
    };
    let escape = |buf: &[u8], at: usize| -> Option<u16> {
        (buf.get(at..at + 2)? == b"\\u").then(|| hex(buf, at + 2))?
    };
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != b'\\' {
            i += 1;
            continue;
        }
        let Some(unit) = escape(buf, i) else {
            // Another escape, like `\\`, whose second byte mustn't start one.
            i += 2;
            continue;
        };
        let paired = (0xD800..0xDC00).contains(&unit)
            && escape(buf, i + 6).is_some_and(|low| (0xDC00..0xE000).contains(&low));
        if paired {
            i += 12;
        } else {
            if (0xD800..0xE000).contains(&unit) {
                buf[i + 2..i + 6].copy_from_slice(b"fffd");
            }
            i += 6;
        }
    }
}

pub(crate) struct Header {
    pub id: String,
    pub cwd: PathBuf,
    pub timestamp: DateTime<Utc>,
    pub version: u32,
    pub len: u64,
}

pub(crate) fn read_header(
    reader: &mut BufReader<File>,
    buf: &mut Vec<u8>,
    path: &Path,
) -> Result<Option<Header>> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        cwd: PathBuf,
        timestamp: DateTime<Utc>,
        // Version 1 files have none (`docs/session-format.md`).
        #[serde(default = "first_version")]
        version: u32,
    }
    fn first_version() -> u32 {
        1
    }
    let Some(len) = read_line(reader, buf, path)? else {
        return Ok(None);
    };
    Ok(serde_json::from_slice::<Line>(buf)
        .ok()
        .filter(|line| line.kind == "session")
        .map(|line| Header {
            id: line.id,
            cwd: line.cwd,
            timestamp: line.timestamp,
            version: line.version,
            len,
        }))
}
