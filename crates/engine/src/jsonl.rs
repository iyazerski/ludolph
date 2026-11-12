//! Reading pi's session files, which pi only appends to or rewrites whole.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Open `path` with its size and modification time.
pub(crate) fn open(path: &Path) -> Result<(BufReader<File>, u64, SystemTime)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    Ok((BufReader::new(file), meta.len(), meta.modified()?))
}

/// How far a reader got through a session file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark<'a> {
    /// The header's id and format version, which a rewrite may change.
    pub id: &'a str,
    pub version: u32,
    /// The end of the last complete line read.
    pub offset: u64,
    /// The file's size and modification time at the last read.
    pub len: u64,
    pub modified: SystemTime,
}

/// What happened to a session file since a [`Mark`].
pub(crate) enum Reopen {
    Unchanged,
    /// pi rewrote it, so it has to be read from the start.
    Rewritten,
    /// It grew; the reader is at the mark's offset.
    Appended {
        reader: BufReader<File>,
        len: u64,
        modified: SystemTime,
    },
}

/// Open `path` again to read what was appended since `mark`.
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

/// Read one complete line into `buf` and return its length in bytes.
///
/// Returns `None` at the end of the file, and for a last line without its
/// newline: pi may be writing it, so it's read once it's complete.
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

/// Replace lone UTF-16 surrogate escapes in the JSON in `buf` with U+FFFD,
/// in place, so it parses.
///
/// pi's `JSON.stringify` writes a lone surrogate, like half an emoji cut off
/// a long text, as `\udXXX`. That's valid JSON, but not valid Unicode, so
/// serde refuses the string. Both escapes are six bytes long.
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

/// A session file's first line.
pub(crate) struct Header {
    pub id: String,
    pub cwd: PathBuf,
    pub timestamp: DateTime<Utc>,
    pub version: u32,
    /// The header line's length in bytes.
    pub len: u64,
}

/// Read the first line as a session header; `None` if it isn't one.
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
