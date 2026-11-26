//! Explicit, bounded context imports. File discovery never reads file contents.
use super::*;
use crate::chatstore::composer::ComposerCommand;
use ghost_protocol::{
    PromptPart,
    composer::{ComposerAttachment, ComposerInput, FileMention},
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::Component,
};

const MAX_IMPORT: usize = 12 * 1024 * 1024;
const MAX_TEXT: usize = 256 * 1024;
const MAX_ITEMS: usize = 8;
/// The longer-side limits, in pixels, a prepared image steps down through until its PNG fits
/// the image attachment limit.
const IMAGE_EDGES: [u32; 4] = [2048, 1600, 1280, 1024];

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::OptionInvalid(message.into())
}

impl SessionManager {
    /// Reads only after the user selects a project file. Never adds its contents to a prompt.
    pub async fn project_file_preview(
        &self,
        chat_id: &str,
        relative: String,
    ) -> Result<ghost_protocol::composer::ProjectFilePreview, SessionError> {
        let folder = self.composer_folder(chat_id).await?;
        self.deps()
            .rt
            .spawn_blocking(move || preview_file(&folder, &relative))
            .await
            .map_err(|_| invalid("The file preview could not be read."))?
    }

    pub async fn composer_draft(
        &self,
        chat_id: &str,
    ) -> Result<ghost_protocol::composer::ComposerDraft, SessionError> {
        self.composer_folder(chat_id).await?;
        let reply = self
            .deps()
            .store
            .composer_command(ComposerCommand::ReadDraft {
                chat_id: chat_id.into(),
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
        let crate::chatstore::composer::ComposerReply::Draft(input) = reply else {
            return Err(invalid("The saved draft could not be read."));
        };
        let files = self
            .deps()
            .store
            .composer(ComposerCommand::Resolve {
                chat_id: chat_id.into(),
                ids: input.attachments.clone(),
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
        Ok(ghost_protocol::composer::ComposerDraft {
            input,
            files: files.into_iter().map(|item| item.meta).collect(),
        })
    }

    pub async fn composer_save_draft(
        &self,
        chat_id: &str,
        input: ComposerInput,
    ) -> Result<(), SessionError> {
        let limit = self.deps().limits.max_prompt_bytes;
        if input.text.len() > limit
            || input.attachments.len() > MAX_ITEMS
            || input.mentions.len() > 32
            || input.attachments.iter().any(|id| id.len() > 128)
            || input.mentions.iter().any(|path| path.len() > 4096)
        {
            return Err(invalid("The draft exceeds the message context limit."));
        }
        self.composer_folder(chat_id).await?;
        self.deps()
            .store
            .composer(ComposerCommand::Resolve {
                chat_id: chat_id.into(),
                ids: input.attachments.clone(),
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
        self.deps()
            .store
            .composer_command(ComposerCommand::SaveDraft {
                chat_id: chat_id.into(),
                input,
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
        Ok(())
    }

    /// Only call after an explicit file picker, drop or paste action by the user.
    pub async fn composer_import(
        &self,
        chat_id: &str,
        path: PathBuf,
    ) -> Result<ComposerAttachment, SessionError> {
        self.composer_folder(chat_id).await?;
        let (name, mime, bytes) = self
            .deps()
            .rt
            .spawn_blocking(move || import_file(&path))
            .await
            .map_err(|_| invalid("The attachment could not be prepared."))??;
        self.composer_store(chat_id, name, mime, bytes).await
    }

    /// Clipboard images have no source path; the same image limits and normalization still apply.
    pub async fn composer_paste(
        &self,
        chat_id: &str,
        bytes: Vec<u8>,
        name: Option<String>,
    ) -> Result<ComposerAttachment, SessionError> {
        // A plain PNG file name of the page's own (the canvas names its drawing); anything else
        // keeps the pasted image's name.
        let name = name
            .filter(|name| {
                name.len() <= 64
                    && name.to_ascii_lowercase().ends_with(".png")
                    && name.len() > 4
                    && !name
                        .chars()
                        .any(|c| c.is_control() || r#"/\:*?"<>|"#.contains(c))
            })
            .unwrap_or_else(|| "Pasted image.png".into());
        self.composer_folder(chat_id).await?;
        let bytes = self
            .deps()
            .rt
            .spawn_blocking(move || normalize_image(&bytes))
            .await
            .map_err(|_| invalid("The image could not be prepared."))??;
        self.composer_store(chat_id, name, "image/png".into(), bytes)
            .await
    }

    /// A photo the person sent from a paired device, into a chat's message box: one
    /// [`prepare_photo`] made is kept as it is, so it is not made twice; anything else is prepared
    /// first. Only after the person's own send from that device.
    pub async fn composer_photo(
        &self,
        chat_id: &str,
        bytes: Vec<u8>,
        name: String,
    ) -> Result<ComposerAttachment, SessionError> {
        self.composer_folder(chat_id).await?;
        let photo = self
            .deps()
            .rt
            .spawn_blocking(move || match prepared(&bytes) {
                Some(mime) => Ok(Photo {
                    name: photo_name(&name, extension(mime)),
                    mime,
                    bytes,
                }),
                None => prepare_photo(&bytes, &name),
            })
            .await
            .map_err(|_| invalid("The image could not be prepared."))??;
        self.composer_store(chat_id, photo.name, photo.mime.into(), photo.bytes)
            .await
    }

    async fn composer_store(
        &self,
        chat_id: &str,
        name: String,
        mime: String,
        bytes: Vec<u8>,
    ) -> Result<ComposerAttachment, SessionError> {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        self.deps()
            .store
            .composer(ComposerCommand::Import {
                chat_id: chat_id.into(),
                name,
                mime,
                sha256,
                bytes: bytes.into(),
            })
            .await
            .map_err(|e| invalid(e.to_string()))?
            .pop()
            .map(|item| item.meta)
            .ok_or_else(|| invalid("The attachment could not be saved."))
    }

    pub async fn composer_mentions(
        &self,
        chat_id: &str,
        query: String,
    ) -> Result<Vec<FileMention>, SessionError> {
        if query.len() > 256 {
            return Err(invalid("Search with a shorter file name."));
        }
        let folder = self.composer_folder(chat_id).await?;
        self.deps()
            .rt
            .spawn_blocking(move || search(&folder, &query))
            .await
            .map_err(|_| invalid("The project files could not be searched."))?
    }

    pub async fn composer_send(
        &self,
        chat_id: &str,
        input: ComposerInput,
    ) -> Result<PromptAck, SessionError> {
        let (text, parts) = self.prepare_composer_input(chat_id, input).await?;
        self.ask(chat_id, |reply| ChatMsg::Send {
            text: text.clone(),
            parts: parts.clone(),
            reply,
        })
        .await
    }

    pub async fn composer_steer(
        &self,
        chat_id: &str,
        expected_turn_id: String,
        input: ComposerInput,
    ) -> Result<ghost_protocol::composer::SteeringAck, SessionError> {
        let (text, parts) = self.prepare_composer_input(chat_id, input).await?;
        self.ask(chat_id, |reply| ChatMsg::Steer {
            expected_turn_id: expected_turn_id.clone(),
            text: text.clone(),
            parts: parts.clone(),
            reply,
        })
        .await
    }

    async fn prepare_composer_input(
        &self,
        chat_id: &str,
        input: ComposerInput,
    ) -> Result<(String, Vec<PromptPart>), SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        if input.attachments.len() > MAX_ITEMS || input.mentions.len() > 32 {
            return Err(invalid(
                "Send up to eight attachments and 32 file mentions at a time.",
            ));
        }
        let limit = self.deps().limits.max_prompt_bytes;
        if input.text.len() > limit {
            return Err(SessionError::PromptTooLarge {
                bytes: input.text.len(),
                limit,
            });
        }
        if input.attachments.iter().any(|id| id.len() > 128) {
            return Err(invalid("An attachment identifier is invalid."));
        }
        let folder = self.composer_folder(chat_id).await?;
        let dir = self.deps().store.attachment_dir().to_path_buf();
        let attachments = self
            .deps()
            .store
            .composer(ComposerCommand::Resolve {
                chat_id: chat_id.into(),
                ids: input.attachments,
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
        let (text, parts) = self
            .deps()
            .rt
            .spawn_blocking(move || {
                let mut display = input.text.clone();
                // Keep the user's editable text distinct from attached file contents, including
                // attachment-only prompts. Continuations preserve every subsequent part verbatim.
                let mut parts = vec![PromptPart::Text { text: input.text }];
                let mut payload_bytes = display.len();
                let mut image_bytes = 0usize;
                let mut seen = HashSet::new();
                for attachment in attachments {
                    if !seen.insert(attachment.meta.id) {
                        continue;
                    }
                    let bytes = ghost_drivers::attachments::read(
                        &dir,
                        &attachment.sha256,
                        if attachment.meta.image {
                            ghost_drivers::attachments::MAX_IMAGE_BYTES
                        } else {
                            MAX_TEXT
                        },
                    )
                    .map_err(invalid)?;
                    if attachment.meta.image {
                        image_bytes += bytes.len();
                        if image_bytes > 8 * 1024 * 1024 {
                            return Err(invalid(
                                "The images exceed the 8 MiB message limit. Send fewer images.",
                            ));
                        }
                        display.push_str(&format!("\n\n[Image: {}]", attachment.meta.name));
                        parts.push(PromptPart::Image {
                            blob: attachment.sha256,
                            mime: attachment.meta.mime,
                        });
                    } else {
                        let text = String::from_utf8(bytes).map_err(|_| {
                            invalid("This attachment is no longer valid UTF-8 text.")
                        })?;
                        // The transcript includes exactly the text context the agent receives.
                        let context =
                            format!("\n\nAttached file: {}\n{}", attachment.meta.name, text);
                        payload_bytes += context.len();
                        display.push_str(&context);
                        parts.push(PromptPart::Text { text: context });
                    }
                }
                for mention in input.mentions {
                    let absolute = checked_mention(&folder, &mention)?;
                    let path = absolute.to_string_lossy().into_owned();
                    payload_bytes += path.len() + 2;
                    display.push_str(&format!("\n@{mention}"));
                    parts.push(PromptPart::Mention { path });
                }
                if payload_bytes > limit || display.len() > limit {
                    return Err(SessionError::PromptTooLarge {
                        bytes: payload_bytes.max(display.len()),
                        limit,
                    });
                }
                Ok((display, parts))
            })
            .await
            .map_err(|_| invalid("The message context could not be prepared."))??;
        Ok((text, parts))
    }

    pub(super) async fn composer_folder(&self, chat_id: &str) -> Result<PathBuf, SessionError> {
        self.deps()
            .store
            .chats()
            .await
            .map_err(|e| SessionError::StorageUnavailable(e.to_string()))?
            .into_iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.folder)
            .ok_or_else(|| SessionError::ChatNotFound(chat_id.into()))
    }
}

pub(super) fn restricted(path: &Path) -> bool {
    path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".ssh"
                | ".gnupg"
                | ".aws"
                | ".azure"
                | ".codex"
                | ".claude"
                | ".pi"
                | ".omp"
                | ".hermes"
                | "keychains"
                | "credentials"
                | "credentials.json"
                | "auth.json"
                | "auth.toml"
                | ".npmrc"
                | ".netrc"
                | "id_rsa"
                | "id_ed25519"
        ) || name == ".env"
            || name.starts_with(".env.")
            || name.ends_with(".key")
            || name.ends_with(".pem")
            || name.ends_with(".p12")
            || name.ends_with(".keychain-db")
    })
}

fn import_file(path: &Path) -> Result<(String, String, Vec<u8>), SessionError> {
    if restricted(path) {
        return Err(invalid(
            "Credential and private-key files cannot be attached.",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| invalid("This file is no longer available."))?;
    if restricted(&canonical) {
        return Err(invalid(
            "Credential and private-key files cannot be attached.",
        ));
    }
    let meta = fs::metadata(&canonical).map_err(|_| invalid("This file could not be opened."))?;
    if !meta.is_file() || meta.len() > MAX_IMPORT as u64 {
        return Err(invalid("Attach a regular file smaller than 12 MiB."));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    fs::File::open(&canonical)
        .and_then(|file| file.take(MAX_IMPORT as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| invalid("This file could not be read."))?;
    if bytes.len() > MAX_IMPORT {
        return Err(invalid("The attachment exceeds 12 MiB."));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.chars().any(char::is_control))
        .ok_or_else(|| invalid("Choose a file with a readable name."))?
        .to_owned();
    if name.len() > 255 {
        return Err(invalid("The attachment name is too long."));
    }
    if image::guess_format(&bytes).is_ok() {
        return Ok((name, "image/png".into(), normalize_image(&bytes)?));
    }
    if bytes.len() > MAX_TEXT || std::str::from_utf8(&bytes).is_err() || bytes.contains(&0) {
        return Err(invalid(
            "Attach a PNG, JPEG, WebP, GIF, or UTF-8 text file under 256 KiB.",
        ));
    }
    Ok((name, "text/plain".into(), bytes))
}

pub(super) fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, SessionError> {
    if bytes.len() > MAX_IMPORT {
        return Err(invalid("The image exceeds 12 MiB."));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("The image format is not supported."))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| invalid("This image is damaged, too large, or not supported."))?;
    let png = |image: &image::DynamicImage| {
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .map_err(|_| invalid("The image could not be prepared."))?;
        Ok::<_, SessionError>(encoded.into_inner())
    };
    // The longer side steps down until the PNG fits. An image within several of the limits is
    // encoded as it is only once.
    let longer = decoded.width().max(decoded.height());
    let mut encoded_as_is = false;
    for edge in IMAGE_EDGES {
        let bytes = if longer > edge {
            png(&decoded.resize(edge, edge, image::imageops::FilterType::Triangle))?
        } else if encoded_as_is {
            continue;
        } else {
            encoded_as_is = true;
            png(&decoded)?
        };
        if bytes.len() <= ghost_drivers::attachments::MAX_IMAGE_BYTES {
            return Ok(bytes);
        }
    }
    Err(invalid(
        "The prepared image exceeds 4 MiB. Use a smaller image.",
    ))
}

/// A photo as a chat's message box keeps it ([`prepare_photo`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    /// Its name, with the extension of what it is now.
    pub name: String,
    /// `image/jpeg` or `image/png`.
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// The quality a photo is kept at when it stays a JPEG.
const PHOTO_QUALITY: u8 = 85;
/// The longest stem a photo's name keeps, in bytes: with its extension, well within a name the
/// message box shows.
const PHOTO_STEM: usize = 56;

/// A photo the person sent from a paired device, as a chat's message box keeps it. A JPEG stays a
/// JPEG (a phone's photos stay small, so a message holds several) and anything else becomes a
/// PNG. Either is decoded and made again, so nothing but its pixels is kept (no location, no
/// camera data), turned upright as its camera recorded it, and stepped down through
/// [`IMAGE_EDGES`] until it is under 4 MiB. `name` names it, with the extension of what it is
/// now.
pub fn prepare_photo(bytes: &[u8], name: &str) -> Result<Photo, SessionError> {
    use image::ImageDecoder as _;
    if bytes.len() > MAX_IMPORT {
        return Err(invalid("The image exceeds 12 MiB."));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("The image format is not supported."))?;
    let jpeg = reader.format() == Some(image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let damaged = || invalid("This image is damaged, too large, or not supported.");
    let mut decoder = reader.into_decoder().map_err(|_| damaged())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|_| damaged())?;
    decoded.apply_orientation(orientation);
    let encode = |image: &image::DynamicImage| {
        let mut encoded = Cursor::new(Vec::new());
        let written =
            if jpeg {
                image.to_rgb8().write_with_encoder(
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, PHOTO_QUALITY),
                )
            } else {
                image.write_to(&mut encoded, image::ImageFormat::Png)
            };
        written.map_err(|_| invalid("The image could not be prepared."))?;
        Ok::<_, SessionError>(encoded.into_inner())
    };
    let longer = decoded.width().max(decoded.height());
    let mut encoded_as_is = false;
    for edge in IMAGE_EDGES {
        let bytes = if longer > edge {
            encode(&decoded.resize(edge, edge, image::imageops::FilterType::Triangle))?
        } else if encoded_as_is {
            continue;
        } else {
            encoded_as_is = true;
            encode(&decoded)?
        };
        if bytes.len() <= ghost_drivers::attachments::MAX_IMAGE_BYTES {
            let mime = if jpeg { "image/jpeg" } else { "image/png" };
            return Ok(Photo {
                name: photo_name(name, extension(mime)),
                mime,
                bytes,
            });
        }
    }
    Err(invalid(
        "The prepared image exceeds 4 MiB. Use a smaller image.",
    ))
}

fn extension(mime: &str) -> &'static str {
    if mime == "image/jpeg" { "jpg" } else { "png" }
}

/// Whether `bytes` are a photo [`prepare_photo`] made, and its type: exactly what the PNG or
/// baseline JPEG encoder writes (no chunk or segment that could carry anything but pixels), at
/// most 2048 pixels a side and 4 MiB. Anything else is prepared again.
fn prepared(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() > ghost_drivers::attachments::MAX_IMAGE_BYTES {
        return None;
    }
    let edge = |width: u32, height: u32| {
        (1..=IMAGE_EDGES[0]).contains(&width) && (1..=IMAGE_EDGES[0]).contains(&height)
    };
    let be16 = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // IHDR first, then IDAT, then IEND at the very end: nothing else.
        let (mut at, mut header, mut data) = (8, false, false);
        while at < bytes.len() {
            let length = usize::try_from(be32(at)?).ok()?;
            let kind = bytes.get(at + 4..at + 8)?;
            let next = at.checked_add(12)?.checked_add(length)?;
            if next > bytes.len() {
                return None;
            }
            match kind {
                b"IHDR" if !header => {
                    if length != 13 || !edge(be32(at + 8)?, be32(at + 12)?) {
                        return None;
                    }
                    header = true;
                }
                b"IDAT" if header => data = true,
                b"IEND" if data && next == bytes.len() => return Some("image/png"),
                _ => return None,
            }
            at = next;
        }
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // JFIF, the frame, its tables, one scan: no segment for a location, a camera or words.
        let mut at = 2;
        let mut frame = false;
        loop {
            if *bytes.get(at)? != 0xFF {
                return None;
            }
            let marker = *bytes.get(at + 1)?;
            let length = usize::from(be16(at + 2)?);
            if length < 2 {
                return None;
            }
            match marker {
                // The plain JFIF header, with no thumbnail.
                0xE0 if length == 16 && bytes.get(at + 4..at + 9)? == b"JFIF\0" => {}
                0xDB | 0xC4 => {}
                0xC0 => {
                    if frame || !edge(u32::from(be16(at + 7)?), u32::from(be16(at + 5)?)) {
                        return None;
                    }
                    frame = true;
                }
                0xDA if frame => {
                    at = at.checked_add(2)?.checked_add(length)?;
                    break;
                }
                _ => return None,
            }
            at = at.checked_add(2)?.checked_add(length)?;
        }
        // The scan's data: stuffed bytes and restarts only, then the end at the very end.
        while at + 1 < bytes.len() {
            if bytes[at] == 0xFF {
                match bytes[at + 1] {
                    0x00 | 0xD0..=0xD7 => at += 2,
                    0xD9 if at + 2 == bytes.len() => return Some("image/jpeg"),
                    _ => return None,
                }
            } else {
                at += 1;
            }
        }
        return None;
    }
    None
}

/// A photo's name as the message box shows it: the stem of the name it came with, with no
/// character a file name cannot hold and no dots or spaces at its ends, and `extension`.
/// "Photo" when nothing is left.
fn photo_name(name: &str, extension: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match base.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => base,
    };
    let clean: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?"<>|"#.contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let ends = |c: char| c == '.' || c.is_whitespace();
    let clean = clean.trim_matches(ends);
    let mut end = clean.len().min(PHOTO_STEM);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    let clean = clean[..end].trim_end_matches(ends);
    format!(
        "{}.{extension}",
        if clean.is_empty() { "Photo" } else { clean }
    )
}

fn checked_mention(folder: &Path, relative: &str) -> Result<PathBuf, SessionError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.len() > 4096
        || relative.chars().any(char::is_control)
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || restricted(path)
    {
        return Err(invalid("Choose a file or folder inside this project."));
    }
    let root = folder
        .canonicalize()
        .map_err(|_| invalid("The project folder is unavailable."))?;
    let target = root
        .join(path)
        .canonicalize()
        .map_err(|_| invalid("The mentioned file or folder is no longer available."))?;
    if !target.starts_with(&root) || restricted(&target) {
        return Err(invalid("The mentioned file must stay inside this project."));
    }
    Ok(target)
}

fn preview_file(
    folder: &Path,
    relative: &str,
) -> Result<ghost_protocol::composer::ProjectFilePreview, SessionError> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    const LIMIT: usize = 128 * 1024;
    if Path::new(relative)
        .components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
    {
        return Err(invalid(
            "Git's internal files are not shown in the project viewer.",
        ));
    }
    let path = checked_mention(folder, relative)?;
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return Err(invalid("Choose a regular project file."));
    }
    // Match checkpoint traversal: every component is capability-relative and refuses links.
    let mut directory = cap_std::fs::Dir::open_ambient_dir(folder, cap_std::ambient_authority())
        .map_err(|_| invalid("The project folder is unavailable."))?;
    let relative_path = Path::new(relative);
    let mut parts = relative_path.components().peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        directory = directory
            .open_dir_nofollow(part.as_os_str())
            .map_err(|_| invalid("The file's parent folder changed or is a link."))?;
    }
    let name = relative_path
        .file_name()
        .ok_or_else(|| invalid("Choose a project file."))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| invalid("The file changed, is a link, or could not be opened."))?;
    let metadata = file
        .metadata()
        .map_err(|_| invalid("The file could not be inspected."))?;
    if !metadata.is_file() {
        return Err(invalid("Choose a regular project file."));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("The file could not be read."))?;
    let truncated = bytes.len() > LIMIT;
    bytes.truncate(LIMIT);
    let text = if bytes.contains(&0) {
        None
    } else {
        match std::str::from_utf8(&bytes) {
            Ok(text) => Some(text.to_owned()),
            Err(error) if truncated && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()])
                    .ok()
                    .map(str::to_owned)
            }
            Err(_) => None,
        }
    };
    Ok(ghost_protocol::composer::ProjectFilePreview {
        path: relative.replace('\\', "/"),
        bytes: metadata.len(),
        text,
        truncated,
    })
}

fn search(folder: &Path, query: &str) -> Result<Vec<FileMention>, SessionError> {
    let root = folder
        .canonicalize()
        .map_err(|_| invalid("The project folder is unavailable."))?;
    let query = query.to_lowercase();
    let mut stack = vec![root.clone()];
    let mut matches = Vec::new();
    let started = Instant::now();
    let mut visited = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 20_000 || started.elapsed() > Duration::from_millis(250) {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(name.as_str(), "node_modules" | "target" | "dist" | "vendor")
            {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if restricted(&path) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative.to_lowercase().contains(&query) {
                matches.push(FileMention {
                    path: relative,
                    directory: kind.is_dir(),
                });
            }
            if kind.is_dir() && path.components().count() < root.components().count() + 12 {
                stack.push(path);
            }
            if matches.len() >= 100 {
                break;
            }
        }
        if visited > 20_000
            || started.elapsed() > Duration::from_millis(250)
            || matches.len() >= 100
        {
            break;
        }
    }
    matches.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_of(image: &image::RgbImage, quality: u8) -> Vec<u8> {
        let mut jpeg = Vec::new();
        image
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut jpeg, quality,
            ))
            .expect("jpeg");
        jpeg
    }

    fn png_of(image: &image::DynamicImage) -> Vec<u8> {
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        png.into_inner()
    }

    /// A JPEG as a phone sends one: an Exif segment first, naming the camera's orientation.
    fn camera_jpeg(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let pixels = image::RgbImage::from_fn(width, height, |x, _| {
            image::Rgb([u8::try_from(x * 5 % 256).expect("byte"), 40, 40])
        });
        let jpeg = jpeg_of(&pixels, 90);
        let mut exif = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
        exif.extend_from_slice(&1u16.to_be_bytes());
        exif.extend_from_slice(&0x0112u16.to_be_bytes());
        exif.extend_from_slice(&3u16.to_be_bytes());
        exif.extend_from_slice(&1u32.to_be_bytes());
        exif.extend_from_slice(&orientation.to_be_bytes());
        exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&u16::try_from(exif.len() + 2).expect("short").to_be_bytes());
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    /// A phone's photo stays a JPEG, turned upright as its camera recorded, with nothing but its
    /// pixels kept; anything else becomes a PNG. What comes out is kept as it is next time.
    #[test]
    fn a_phones_photo_stays_a_jpeg_upright_with_only_its_pixels() {
        let sent = camera_jpeg(40, 20, 6);
        assert_eq!(prepared(&sent), None, "its Exif segment: made again");
        let photo = prepare_photo(&sent, "IMG_0001.HEIC").expect("photo");
        assert_eq!(
            (photo.name.as_str(), photo.mime),
            ("IMG_0001.jpg", "image/jpeg")
        );
        let upright = image::load_from_memory(&photo.bytes).expect("jpeg");
        assert_eq!((upright.width(), upright.height()), (20, 40), "turned");
        assert!(!photo.bytes.windows(4).any(|window| window == b"Exif"));
        assert_eq!(prepared(&photo.bytes), Some("image/jpeg"));
        let again = prepare_photo(&photo.bytes, &photo.name).expect("again");
        assert_eq!(again.name, "IMG_0001.jpg");

        let screen = png_of(&image::DynamicImage::new_rgba8(30, 10));
        let photo = prepare_photo(&screen, "Screen Shot.png").expect("png");
        assert_eq!(
            (photo.name.as_str(), photo.mime),
            ("Screen Shot.png", "image/png")
        );
        assert_eq!(prepared(&photo.bytes), Some("image/png"));
        let mut gif = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(4, 4)
            .write_to(&mut gif, image::ImageFormat::Gif)
            .expect("gif");
        let photo = prepare_photo(gif.get_ref(), "moving.gif").expect("gif");
        assert_eq!(
            (photo.name.as_str(), photo.mime),
            ("moving.png", "image/png")
        );
        assert!(prepare_photo(b"not an image", "a.jpg").is_err());
    }

    /// A photo larger than an image may be steps down until it fits, and stays a JPEG.
    #[test]
    fn a_large_photo_steps_down_until_it_fits() {
        let mut seed = 11u32;
        let noise = image::RgbImage::from_fn(2400, 2400, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [a, b, c, _] = seed.to_le_bytes();
            image::Rgb([a, b, c])
        });
        let sent = jpeg_of(&noise, 85);
        let photo = prepare_photo(&sent, "noise.jpg").expect("fits");
        assert!(photo.bytes.len() <= ghost_drivers::attachments::MAX_IMAGE_BYTES);
        let back = image::load_from_memory(&photo.bytes).expect("jpeg");
        assert!(back.width() < 2048, "{}", back.width());
        assert_eq!(photo.mime, "image/jpeg");
    }

    /// Only what the encoders write is kept as it is: a segment or a chunk that could carry
    /// anything but pixels, a side over 2048, a broken or padded file, all are made again.
    #[test]
    fn only_what_prepare_photo_makes_is_kept_as_it_is() {
        let jpeg = jpeg_of(&image::RgbImage::new(16, 8), 85);
        assert_eq!(prepared(&jpeg), Some("image/jpeg"));
        let with = |at: usize, insert: &[u8]| [&jpeg[..at], insert, &jpeg[at..]].concat();
        for (case, bytes) in [
            ("a comment", with(2, &[0xFF, 0xFE, 0, 4, b'h', b'i'])),
            ("an Exif segment", with(2, &[0xFF, 0xE1, 0, 4, 0, 0])),
            ("bytes after the end", [jpeg.as_slice(), b"tail"].concat()),
            ("cut short", jpeg[..jpeg.len() - 2].to_vec()),
            (
                "wider than 2048",
                jpeg_of(&image::RgbImage::new(2049, 1), 85),
            ),
        ] {
            assert_eq!(prepared(&bytes), None, "{case}");
        }
        let png = png_of(&image::DynamicImage::new_rgb8(16, 8));
        assert_eq!(prepared(&png), Some("image/png"));
        let text = {
            let mut chunk = 5u32.to_be_bytes().to_vec();
            chunk.extend_from_slice(b"tEXtwords\0\0\0\0");
            chunk
        };
        for (case, bytes) in [
            (
                "a text chunk",
                [&png[..33], text.as_slice(), &png[33..]].concat(),
            ),
            ("bytes after the end", [png.as_slice(), b"tail"].concat()),
            ("no data", [&png[..33], &png[png.len() - 12..]].concat()),
            (
                "wider than 2048",
                png_of(&image::DynamicImage::new_rgb8(2049, 1)),
            ),
        ] {
            assert_eq!(prepared(&bytes), None, "{case}");
        }
        assert_eq!(prepared(b"GIF89a"), None);
        assert_eq!(
            prepared(&vec![0; ghost_drivers::attachments::MAX_IMAGE_BYTES + 1]),
            None
        );
    }

    /// A photo keeps its own name's stem, as a file name can hold it, with the extension of what
    /// it is now; one with nothing left (dots, spaces) is "Photo".
    #[test]
    fn a_photos_name_keeps_its_stem_with_the_extension_of_what_it_is() {
        for (name, extension, kept) in [
            ("IMG_0001.HEIC", "jpg", "IMG_0001.jpg"),
            ("Screen Shot.jpeg", "png", "Screen Shot.png"),
            ("x a:b*c?\u{7}.jpg", "jpg", "x a-b-c--.jpg"),
            ("photos/2026\\IMG_2.jpg", "jpg", "IMG_2.jpg"),
            (".hidden.jpg", "jpg", "hidden.jpg"),
            ("", "jpg", "Photo.jpg"),
            ("...", "png", "Photo.png"),
            ("  .  ", "png", "Photo.png"),
            ("plain", "png", "plain.png"),
        ] {
            assert_eq!(photo_name(name, extension), kept, "{name:?}");
        }
        let long = photo_name(&format!("{}.jpg", "é".repeat(60)), "jpg");
        assert!(
            long.len() <= PHOTO_STEM + 4 && long.ends_with(".jpg"),
            "{long}"
        );
        assert!(long.starts_with("éé"));
    }

    #[test]
    fn file_preview_bounds_text_and_refuses_private_paths() {
        let temp = tempfile::tempdir().expect("project");
        fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("source");
        let source = preview_file(temp.path(), "main.rs").expect("preview");
        assert_eq!(source.text.as_deref(), Some("fn main() {}\n"));
        assert!(!source.truncated);
        fs::write(temp.path().join("large.txt"), "é".repeat(100_000)).expect("large");
        let large = preview_file(temp.path(), "large.txt").expect("large preview");
        assert!(large.truncated);
        assert!(large.text.expect("valid text").len() <= 128 * 1024);
        fs::write(temp.path().join("binary.dat"), [0, 255]).expect("binary");
        assert!(
            preview_file(temp.path(), "binary.dat")
                .expect("binary preview")
                .text
                .is_none()
        );
        assert!(preview_file(temp.path(), "../outside").is_err());
        assert!(preview_file(temp.path(), ".git/config").is_err());
        assert!(preview_file(temp.path(), "auth.json").is_err());
    }

    #[test]
    fn imports_normalize_images_and_reject_binary_or_oversized_text() {
        let temp = tempfile::tempdir().expect("folder");
        let path = temp.path().join("sample.png");
        let source = image::DynamicImage::new_rgba8(2400, 10);
        source.save(&path).expect("synthetic image");
        let (_, mime, bytes) = import_file(&path).expect("import image");
        assert_eq!(mime, "image/png");
        let image = image::load_from_memory(&bytes).expect("normalized");
        assert_eq!(image.width(), 2048);
        assert!(image.height() <= 10);
        let text = temp.path().join("sample.txt");
        fs::write(&text, "visible context").expect("text");
        assert_eq!(
            import_file(&text).expect("import text").2,
            b"visible context"
        );
        fs::write(&text, [0, 1, 2, 3]).expect("binary");
        assert!(import_file(&text).is_err());
        fs::write(&text, vec![b'a'; MAX_TEXT + 1]).expect("long text");
        assert!(import_file(&text).is_err());
    }

    /// Deterministic noise (xorshift64) that no PNG filter or compression can shrink.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect()
    }

    fn png(image: image::DynamicImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("fixture PNG");
        bytes.into_inner()
    }

    #[test]
    fn a_large_image_steps_down_until_its_png_fits() {
        // Too large as a PNG at its own size (within 2048) and at 1600; it fits at 1280.
        let source =
            image::RgbImage::from_raw(2000, 1500, noise(2000 * 1500 * 3)).expect("noise image");
        let bytes = png(image::DynamicImage::ImageRgb8(source));
        assert!(bytes.len() > ghost_drivers::attachments::MAX_IMAGE_BYTES);
        assert!(bytes.len() <= MAX_IMPORT);
        let prepared = normalize_image(&bytes).expect("stepped down");
        assert!(prepared.len() <= ghost_drivers::attachments::MAX_IMAGE_BYTES);
        let image = image::load_from_memory(&prepared).expect("prepared PNG");
        assert_eq!((image.width(), image.height()), (1280, 960));
    }

    #[test]
    fn an_image_too_large_at_1024_is_refused() {
        // 1024 square RGBA noise is larger than 4 MiB as a PNG at every limit it is within.
        let source =
            image::RgbaImage::from_raw(1024, 1024, noise(1024 * 1024 * 4)).expect("noise image");
        let error = normalize_image(&png(image::DynamicImage::ImageRgba8(source)))
            .expect_err("still too large at 1024");
        assert_eq!(
            error.to_string(),
            invalid("The prepared image exceeds 4 MiB. Use a smaller image.").to_string()
        );
    }

    #[test]
    fn discovery_skips_private_files_and_mentions_cannot_leave_project() {
        let temp = tempfile::tempdir().expect("folder");
        fs::create_dir(temp.path().join("src")).expect("src");
        fs::create_dir(temp.path().join("node_modules")).expect("dependencies");
        fs::write(temp.path().join("src/main.rs"), "fn main() {}").expect("source");
        fs::write(temp.path().join(".env"), "synthetic fixture only").expect("fixture");
        fs::write(temp.path().join("auth.json"), "synthetic fixture only").expect("fixture");
        let found = search(temp.path(), "").expect("search");
        assert!(found.iter().any(|item| item.path == "src/main.rs"));
        assert!(!found.iter().any(|item| item.path.contains("auth")
            || item.path.contains(".env")
            || item.path.contains("node_modules")));
        assert!(checked_mention(temp.path(), "../outside").is_err());
        assert!(checked_mention(temp.path(), ".env").is_err());
        assert!(checked_mention(temp.path(), "src/main.rs").is_ok());
        // Reject the path before trying to read the synthetic private-file fixture.
        assert!(import_file(&temp.path().join("auth.json")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_mention_symlink_cannot_escape_the_project() {
        let temp = tempfile::tempdir().expect("project");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("file.txt"), "outside").expect("outside file");
        std::os::unix::fs::symlink(outside.path(), temp.path().join("escape")).expect("link");
        assert!(checked_mention(temp.path(), "escape/file.txt").is_err());
        assert!(preview_file(temp.path(), "escape/file.txt").is_err());
        fs::write(temp.path().join("inside.txt"), "inside").expect("inside file");
        std::os::unix::fs::symlink("inside.txt", temp.path().join("alias.txt"))
            .expect("inside link");
        assert!(preview_file(temp.path(), "alias.txt").is_err());
        assert!(
            !search(temp.path(), "")
                .expect("search")
                .iter()
                .any(|item| item.path.contains("escape"))
        );
    }
}
