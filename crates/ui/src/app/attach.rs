use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbaImage};
use rune::{ClipboardContent, Image, TextEdit, Tx};
use serde::Serialize;
use serde_json::json;

use super::{ChatPage, Page, State};
use crate::data::{ChatImage, Role};
use crate::markup;

/// 72-point thumbnails need 144 pixels on Retina displays.
const THUMB_PX: u32 = 144;
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const MAX_EDGE: u32 = 1600;
const KEEP_BYTES: usize = 1 << 20;
const MAX_FILE: u64 = 64 << 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub mime_type: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<Upload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Upload {
    pub id: String,
    pub original: ChatImage,
}

pub struct Attached {
    pub id: u64,
    pub number: usize,
    pub thumb: Option<Image>,
    pub image: Prep,
    pub link: String,
}

pub enum Prep {
    Reading,
    Ready(Payload),
    Failed(String),
}

enum Source {
    Pasted(Arc<[u8]>, String),
    File(PathBuf),
}

impl State {
    pub fn paste(&mut self, content: &ClipboardContent, _: &mut Tx) -> bool {
        if self.chat().is_some_and(|page| page.queue_edit.is_some()) {
            return false;
        }
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
                self.attach(Source::Pasted(
                    image.png().into(),
                    uuid::Uuid::new_v4().to_string(),
                ));
                true
            }
            _ => false,
        }
    }

    fn attach(&mut self, source: Source) {
        self.next_attached += 1;
        let id = self.next_attached;
        let Some(page) = self.chat_mut() else { return };
        let number = page.next_image_number();
        let target = match &source {
            Source::Pasted(_, id) => format!("attachment:{id}").into(),
            Source::File(path) => path.to_string_lossy(),
        };
        let link = markup::link(&format!("Image {number}"), &target);
        insert_spaced(&mut page.composer, &link);
        page.attached.push(Attached {
            id,
            number,
            thumb: None,
            image: Prep::Reading,
            link,
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
                let Some(attached) = s.attachment_mut(id) else {
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

    fn attachment_mut(&mut self, id: u64) -> Option<&mut Attached> {
        let page = match &mut self.page {
            Page::Chat(page) => Some(page),
            _ => None,
        };
        let current = page.into_iter().flat_map(|page| {
            page.attached.iter_mut().chain(
                page.queue_edit
                    .iter_mut()
                    .flat_map(|edit| &mut edit.attached),
            )
        });
        current
            .chain(
                self.drafts
                    .values_mut()
                    .flat_map(|draft| &mut draft.attached),
            )
            .find(|image| image.id == id)
    }

    pub fn detach(&mut self, id: u64) {
        let Some(page) = self.chat_mut() else { return };
        let Some(at) = page.attached.iter().position(|a| a.id == id) else {
            return;
        };
        let link = page.attached.remove(at).link;
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
            |s, path: Result<String, String>, tx| match path {
                Ok(path) => s.open_path(&path, tx),
                Err(e) => s.fail("Couldn't open the image", e),
            },
        );
    }
}

fn prepare(source: &Source) -> Result<(Payload, RgbaImage), String> {
    let (mut payload, thumb) = prepare_image(source)?;
    if let Source::Pasted(bytes, id) = source {
        payload.file = Some(Upload {
            id: id.clone(),
            original: ChatImage {
                mime_type: "image/png".to_owned(),
                data: BASE64.encode(bytes),
            },
        });
    }
    Ok((payload, thumb))
}

fn prepare_image(source: &Source) -> Result<(Payload, RgbaImage), String> {
    let bytes: Arc<[u8]> = match source {
        Source::Pasted(bytes, _) => bytes.clone(),
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
            file: None,
        };
        match decoded {
            Ok(image) if small(&image) => return Ok((keep, thumbnail(&image)?)),
            Ok(_) => {}
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
            file: None,
        },
        thumb,
    ))
}

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
    pub fn next_image_number(&self) -> usize {
        self.attached.iter().map(|a| a.number).max().unwrap_or(0) + 1
    }

    pub fn images_ready(&self) -> bool {
        self.attached
            .iter()
            .all(|a| matches!(a.image, Prep::Ready(_)))
    }
}

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
