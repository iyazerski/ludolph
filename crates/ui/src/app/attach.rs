//! Images in the composer and the transcript. A pasted or copied image is
//! read and, if it's large, shrunk off the UI thread as soon as it's
//! attached, so sending only has to hand over the bytes. The transcript's
//! images load from the engine the first time they show.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbaImage};
use rune::{ClipboardContent, Image, TextEdit, Tx};
use serde::Serialize;
use serde_json::json;

use super::{ChatPage, State};
use crate::data::{ChatImage, Role};
use crate::markup;

/// Thumbnails shrink to this many pixels square: the largest they draw
/// (72 points) on a Retina screen. Smaller draws use the mips.
const THUMB_PX: u32 = 144;
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
/// The longest edge sent, as the phone sends: models shrink larger images
/// anyway, and the bytes go into the session file.
const MAX_EDGE: u32 = 1600;
/// An image this small in a type models take goes as it is.
const KEEP_BYTES: usize = 1 << 20;
/// The largest file read: a screenshot is a few MB.
const MAX_FILE: u64 = 64 << 20;

/// An image as `send_message` and `create_chat` take it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub mime_type: String,
    /// Base64.
    pub data: String,
}

pub struct Attached {
    pub id: u64,
    /// Its label, `Image 2`, unique among the composer's images.
    pub number: usize,
    pub thumb: Option<Image>,
    pub image: Prep,
    /// The link to it put in the text where it was pasted.
    pub link: Option<String>,
}

/// Where an attached image is on its way to being sendable.
pub enum Prep {
    Reading,
    Ready(Payload),
    Failed(String),
}

/// What was pasted or dropped: an image's bytes, with where to save them so
/// the text can link to them, or a file.
enum Source {
    Pasted(Arc<[u8]>, PathBuf),
    File(PathBuf),
}

impl State {
    /// Attach the images a paste carries, each with a link to it at the
    /// caret; any other paste stays text.
    pub fn paste(&mut self, content: &ClipboardContent, _: &mut Tx) -> bool {
        let files = content.files();
        if !files.is_empty() {
            // A copy in Finder also carries the file's icon as its image.
            let images: Vec<PathBuf> = files.iter().filter(|p| is_image(p)).cloned().collect();
            let any = !images.is_empty();
            for path in images {
                self.attach(Source::File(path));
            }
            return any;
        }
        // Office apps put a picture of the copied text beside it.
        match content.image() {
            Some(image) if content.text().trim().is_empty() => {
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let number = self.chat().map_or(1, ChatPage::next_image_number);
                let path = self
                    .api
                    .uploads()
                    .join(format!("{millis}-image-{number}.png"));
                self.attach(Source::Pasted(image.png().into(), path));
                true
            }
            _ => false,
        }
    }

    fn attach(&mut self, source: Source) {
        let Some(page) = self.chat_mut() else { return };
        let (id, nav) = (page.attached_id(), page.nav);
        let number = page.next_image_number();
        let target = match &source {
            Source::Pasted(_, path) | Source::File(path) => path.to_string_lossy(),
        };
        let link = markup::link(&format!("Image {number}"), &target);
        insert_spaced(&mut page.composer, &link);
        page.attached.push(Attached {
            id,
            number,
            thumb: None,
            image: Prep::Reading,
            link: Some(link),
        });
        self.api.run(
            move |_| async move { tokio::task::spawn_blocking(move || prepare(&source)).await },
            move |s, prepared, _| {
                let prepared = prepared.map_err(|e| e.to_string()).and_then(|p| p);
                let thumb = match &prepared {
                    Ok((_, thumb)) => s.images.as_ref().map(|cache| {
                        let (width, height) = thumb.dimensions();
                        cache.rgba(width, height, thumb.as_raw().clone())
                    }),
                    Err(_) => None,
                };
                let Some(attached) = s
                    .page_at(nav)
                    .and_then(|p| p.attached.iter_mut().find(|a| a.id == id))
                else {
                    return;
                };
                attached.thumb = thumb;
                match prepared {
                    Ok((payload, _)) => attached.image = Prep::Ready(payload),
                    Err(e) => {
                        attached.image = Prep::Failed(e.clone());
                        s.fail("Couldn't attach the image", e);
                    }
                }
            },
        );
    }

    /// Put images that didn't send back in the composer of page `nav`.
    pub fn reattach(&mut self, nav: u64, images: Vec<(Payload, Option<Image>)>) {
        let Some(page) = self.page_at(nav) else {
            return;
        };
        for (payload, thumb) in images {
            let id = page.attached_id();
            let number = page.next_image_number();
            page.attached.push(Attached {
                id,
                number,
                thumb,
                image: Prep::Ready(payload),
                link: None,
            });
        }
    }

    /// Remove an attached image, and its link from the text.
    pub fn detach(&mut self, id: u64) {
        let Some(page) = self.chat_mut() else { return };
        let Some(at) = page.attached.iter().position(|a| a.id == id) else {
            return;
        };
        let Some(link) = page.attached.remove(at).link else {
            return;
        };
        let edit = &mut page.composer;
        if let Some(start) = edit.text.find(&link) {
            let end = start + link.len();
            let end = end + usize::from(edit.text[end..].starts_with(' '));
            edit.undo_group(|e| {
                e.set_selection(start, end);
                e.insert("");
            });
        }
    }

    /// Load the shown messages' images the first time they show. A message
    /// pi hasn't written yet waits, since its images have no address.
    pub fn load_thumbs(&mut self) {
        if self.images.is_none() {
            return;
        }
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut() else {
            return;
        };
        let mut new = Vec::new();
        let shown = chat.before.iter().chain(chat.page.iter().flatten());
        for m in shown.filter(|m| m.role == Role::User && m.written()) {
            for index in m.images() {
                let key = (m.id.clone(), index);
                if !chat.thumbs.contains_key(&key) {
                    chat.thumbs.insert(key.clone(), None);
                    new.push(key);
                }
            }
        }
        let id = chat.id.clone();
        for (message, index) in new {
            let args = json!({"chat": id, "message": message, "index": index});
            self.api.run(
                move |client| async move {
                    let answer = client.call("chat_image", args).await?;
                    let image: ChatImage = serde_json::from_value(answer)
                        .map_err(|e| format!("unexpected chat_image answer: {e}"))?;
                    tokio::task::spawn_blocking(move || {
                        let bytes = BASE64.decode(&image.data).map_err(|e| e.to_string())?;
                        thumbnail(&image::load_from_memory(&bytes).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .map_err(|e| e.to_string())?
                },
                move |s, thumb: Result<RgbaImage, String>, _| {
                    let Ok(thumb) = thumb else { return };
                    let Some(cache) = &s.images else { return };
                    let (width, height) = thumb.dimensions();
                    let image = cache.rgba(width, height, thumb.into_raw());
                    if let Some(chat) = s.page_at(nav).and_then(ChatPage::existing_mut) {
                        chat.thumbs.insert((message, index), Some(image));
                    }
                },
            );
        }
    }

    /// Open a transcript image in Preview, from a file in the temporary folder.
    pub fn open_image(&mut self, message: String, index: usize) {
        let Some(chat) = self.chat().and_then(ChatPage::existing) else {
            return;
        };
        let args = json!({"chat": chat.id, "message": message, "index": index});
        self.api.run(
            move |client| async move {
                let answer = client.call("chat_image", args).await?;
                let image: ChatImage = serde_json::from_value(answer)
                    .map_err(|e| format!("unexpected chat_image answer: {e}"))?;
                let bytes = BASE64.decode(&image.data).map_err(|e| e.to_string())?;
                let ext = image.mime_type.rsplit('/').next().unwrap_or("png");
                let path = std::env::temp_dir().join(format!("ludolph-{message}-{index}.{ext}"));
                tokio::fs::write(&path, bytes)
                    .await
                    .map_err(|e| format!("writing {}: {e}", path.display()))?;
                Ok(path.to_string_lossy().into_owned())
            },
            |s, path: Result<String, String>, _| match path {
                Ok(path) => s.open_path(path),
                Err(e) => s.fail("Couldn't open the image", e),
            },
        );
    }
}

/// Read an image and make it sendable: as it is when it's small and in a
/// type models take, else no larger than [`MAX_EDGE`], as JPEG over white.
/// Also return its thumbnail.
fn prepare(source: &Source) -> Result<(Payload, RgbaImage), String> {
    let bytes: Arc<[u8]> = match source {
        Source::Pasted(bytes, save) => {
            let shown = save.display();
            if let Some(dir) = save.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("creating {shown}: {e}"))?;
            }
            std::fs::write(save, bytes).map_err(|e| format!("writing {shown}: {e}"))?;
            bytes.clone()
        }
        Source::File(path) => {
            let shown = path.display();
            let size = std::fs::metadata(path)
                .map_err(|e| format!("reading {shown}: {e}"))?
                .len();
            if size > MAX_FILE {
                return Err(format!("{shown} is too large"));
            }
            std::fs::read(path)
                .map_err(|e| format!("reading {shown}: {e}"))?
                .into()
        }
    };
    let format = image::guess_format(&bytes).map_err(|_| "that isn't an image".to_owned())?;
    let decoded = image::load_from_memory_with_format(&bytes, format).map_err(|e| match format {
        // Only PNG and JPEG decode here; the others go as they are when small.
        ImageFormat::Gif | ImageFormat::WebP => "this image is too large to send".to_owned(),
        _ => e.to_string(),
    });
    let mime = match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    };
    let small = |image: &DynamicImage| image.width().max(image.height()) <= MAX_EDGE;
    if let Some(mime) = mime
        && bytes.len() <= KEEP_BYTES
    {
        let keep = Payload {
            mime_type: mime.to_owned(),
            data: BASE64.encode(&bytes),
        };
        match decoded {
            Ok(image) if small(&image) => return Ok((keep, thumbnail(&image)?)),
            Ok(_) => {}
            // A GIF or WebP can't make a thumbnail here, so it shows a blank tile.
            Err(_) => return Ok((keep, RgbaImage::new(1, 1))),
        }
    }
    let image = decoded?;
    let fitted = if small(&image) {
        image
    } else {
        image.resize(MAX_EDGE, MAX_EDGE, FilterType::Triangle)
    };
    let thumb = thumbnail(&fitted)?;
    let mut white = RgbaImage::from_pixel(fitted.width(), fitted.height(), [255; 4].into());
    image::imageops::overlay(&mut white, &fitted.to_rgba8(), 0, 0);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), 85)
        .encode_image(&DynamicImage::ImageRgba8(white).to_rgb8())
        .map_err(|e| e.to_string())?;
    Ok((
        Payload {
            mime_type: "image/jpeg".to_owned(),
            data: BASE64.encode(&jpeg),
        },
        thumb,
    ))
}

/// Crop an image to a centered square, as the tiles show it.
fn thumbnail(image: &DynamicImage) -> Result<RgbaImage, String> {
    Ok(image
        .resize_to_fill(THUMB_PX, THUMB_PX, FilterType::Triangle)
        .into_rgba8())
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Insert `s` at the caret, apart from the words on either side.
fn insert_spaced(edit: &mut TextEdit, s: &str) {
    let (start, end) = edit.range();
    let before = edit.text[..start].chars().next_back();
    let after = edit.text[end..].chars().next();
    let lead = if before.is_some_and(|c| !c.is_whitespace()) {
        " "
    } else {
        ""
    };
    let trail = if after.is_none_or(|c| !c.is_whitespace()) {
        " "
    } else {
        ""
    };
    edit.insert(&format!("{lead}{s}{trail}"));
}

impl ChatPage {
    /// The next attached image's number: one past the highest shown.
    pub fn next_image_number(&self) -> usize {
        self.attached.iter().map(|a| a.number).max().unwrap_or(0) + 1
    }

    pub fn images_ready(&self) -> bool {
        self.attached
            .iter()
            .all(|a| matches!(a.image, Prep::Ready(_)))
    }
}

/// The attached images with their thumbnails, or why the message can't go yet.
pub fn ready(page: &ChatPage) -> Result<Vec<(Payload, Option<Image>)>, &'static str> {
    page.attached
        .iter()
        .map(|a| match &a.image {
            Prep::Ready(payload) => Ok((payload.clone(), a.thumb.clone())),
            Prep::Reading => Err("The images are still loading."),
            Prep::Failed(_) => Err("An image didn't attach. Remove it to send."),
        })
        .collect()
}
