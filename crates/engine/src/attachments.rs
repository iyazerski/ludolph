use std::collections::HashSet;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use uuid::Uuid;

use crate::index;
use crate::transcript::Image;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(flatten)]
    pub image: Image,
    pub file: Option<Upload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Upload {
    pub id: String,
    pub original: Option<Image>,
}

impl Attachment {
    pub(crate) fn bytes(&self) -> usize {
        self.image.data.len()
            + self
                .file
                .as_ref()
                .and_then(|f| f.original.as_ref())
                .map_or(0, |i| i.data.len())
    }

    fn path(&self, root: &Path, session: &str) -> Result<Option<PathBuf>> {
        let Some(file) = &self.file else {
            return Ok(None);
        };
        let session = Uuid::parse_str(session).context("invalid attachment session id")?;
        let id = Uuid::parse_str(&file.id).context("invalid attachment id")?;
        let image = file.original.as_ref().unwrap_or(&self.image);
        let ext = match image.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            mime => bail!("unsupported attachment type {mime}"),
        };
        Ok(Some(
            root.join(session.to_string()).join(format!("{id}.{ext}")),
        ))
    }
}

/// Resolve draft links without writing files; queued and discarded drafts stay in memory.
pub(crate) fn resolve(
    root: &Path,
    session: &str,
    text: &str,
    images: &[Attachment],
) -> Result<String> {
    let mut text = text.to_owned();
    for (index, image) in images.iter().enumerate() {
        let Some(path) = image.path(root, session)? else {
            continue;
        };
        let Some(file) = &image.file else { continue };
        let marker = format!("](attachment:{})", file.id);
        let target = format!("](<{}>)", path.display());
        if text.contains(&marker) {
            text = text.replace(&marker, &target);
        } else if !text.contains(path.to_string_lossy().as_ref()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("[Image {}{target}", index + 1));
        }
    }
    Ok(text)
}

pub(crate) fn save(root: &Path, session: &str, images: Vec<Attachment>) -> Result<Vec<Image>> {
    // Decode before creating any files, so an invalid payload leaves no partial upload.
    let files = images
        .iter()
        .map(|image| -> Result<_> {
            let Some(path) = image.path(root, session)? else {
                return Ok(None);
            };
            let data = image
                .file
                .as_ref()
                .and_then(|f| f.original.as_ref())
                .unwrap_or(&image.image);
            let bytes = BASE64
                .decode(&data.data)
                .context("invalid attachment image data")?;
            Ok(Some((path, bytes)))
        })
        .collect::<Result<Vec<_>>>()?;
    for (path, bytes) in files.into_iter().flatten() {
        let parent = path.parent().context("attachment has no directory")?;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(&bytes) {
                    let _ = std::fs::remove_file(&path);
                    return Err(error).with_context(|| format!("writing {}", path.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // A rejected queued prompt may be retried with the same image id.
                if std::fs::read(&path)? != bytes {
                    bail!(
                        "attachment {} already contains another image",
                        path.display()
                    );
                }
            }
            Err(error) => return Err(error).with_context(|| format!("writing {}", path.display())),
        }
    }
    Ok(images.into_iter().map(|image| image.image).collect())
}

/// Only run on chat deletion. Check every branch and terminal-created fork before removing files.
/// `protected` is asked after listing the folders, so a chat started meanwhile is either protected
/// or has no folder in the list.
pub(crate) fn cleanup(
    root: &Path,
    sessions: &Path,
    protected: impl FnOnce() -> HashSet<String>,
) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() && Uuid::parse_str(&id).is_ok() {
            candidates.push((id, entry.path()));
        }
    }
    if candidates.is_empty() {
        return Ok(());
    }
    let mut protected = protected();
    candidates.retain(|(id, _)| !protected.contains(id));
    if candidates.is_empty() {
        return Ok(());
    }
    let prefix = format!("{}/", root.display());
    let mut line = String::new();
    for path in index::session_files(sessions)? {
        let mut reader = BufReader::new(
            File::open(&path).with_context(|| format!("reading {}", path.display()))?,
        );
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let mut references = References {
                prefix: &prefix,
                protected: &mut protected,
            };
            (&mut references)
                .deserialize(&mut serde_json::Deserializer::from_str(&line))
                .with_context(|| format!("checking attachment references in {}", path.display()))?;
        }
    }
    for (id, path) in candidates {
        if !protected.contains(&id) {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

// Walk strings without allocating a JSON tree or copying base64 payloads. Serde still
// decodes escaped paths, including those written by terminal extensions and forks.
struct References<'a> {
    prefix: &'a str,
    protected: &'a mut HashSet<String>,
}

impl<'de> DeserializeSeed<'de> for &mut References<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for &mut References<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a pi session entry")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        for (at, _) in value.match_indices(self.prefix) {
            let rest = &value[at + self.prefix.len()..];
            if let Some(id) = rest
                .get(..36)
                .filter(|_| rest.as_bytes().get(36) == Some(&b'/'))
            {
                self.protected.insert(id.to_owned());
            }
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(&mut *self)?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key::<IgnoredAny>()?.is_some() {
            map.next_value_seed(&mut *self)?;
        }
        Ok(())
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}
