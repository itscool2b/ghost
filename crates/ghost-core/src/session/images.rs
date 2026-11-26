//! Chat-scoped image previews. No remote requests, arbitrary attachment hashes or active formats.
use super::*;
use crate::chatstore::composer::{ComposerCommand, ComposerReply};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ghost_protocol::{
    PromptPart,
    composer::{ChatImage, ChatImageContext, ChatImagePreview, ChatImageSource},
};
use std::{
    io::{Cursor, Read},
    path::Component,
};

const MAX_SOURCE_BYTES: usize = 12 * 1024 * 1024;
static DECODES: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

pub(super) struct StoredImages {
    pub store: Arc<crate::chatstore::ChatStore>,
    pub chat_id: String,
    pub rt: tokio::runtime::Handle,
}

impl std::fmt::Debug for StoredImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredImages")
    }
}

impl ghost_drivers::incoming_images::AttachmentWriter for StoredImages {
    fn write(&self, bytes: Vec<u8>) -> futures::future::BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            use sha2::{Digest as _, Sha256};
            if bytes.len() > MAX_SOURCE_BYTES {
                return Err("The image exceeds 12 MiB.".into());
            }
            let permit = DECODES
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| "Image decoding stopped.")?;
            let bytes = self
                .rt
                .spawn_blocking(move || {
                    let _permit = permit;
                    super::composer::normalize_image(&bytes)
                })
                .await
                .map_err(|_| "The image could not be prepared.")?
                .map_err(|error| error.to_string())?;
            let sha256 = format!("{:x}", Sha256::digest(&bytes));
            self.store
                .composer(ComposerCommand::Import {
                    chat_id: self.chat_id.clone(),
                    name: "Agent image.png".into(),
                    mime: "image/png".into(),
                    sha256,
                    bytes: bytes.into(),
                })
                .await
                .map_err(|error| error.to_string())?
                .pop()
                .map(|item| item.sha256)
                .ok_or_else(|| "The image could not be saved.".into())
        })
    }
}

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::OptionInvalid(message.into())
}

fn check_context(context: &ChatImageContext) -> Result<(), SessionError> {
    let id = match context {
        ChatImageContext::Turn { turn_id } => turn_id,
        ChatImageContext::Steering { message_id } => message_id,
        ChatImageContext::Queued { prompt_id } => prompt_id,
    };
    if id.is_empty() || id.len() > 128 {
        return Err(invalid("The message image reference is invalid."));
    }
    Ok(())
}

impl SessionManager {
    async fn image_parts(
        &self,
        chat_id: &str,
        context: ChatImageContext,
    ) -> Result<Vec<PromptPart>, SessionError> {
        check_context(&context)?;
        self.composer_folder(chat_id).await?;
        match self
            .deps()
            .store
            .composer_command(ComposerCommand::ImageParts {
                chat_id: chat_id.into(),
                context,
            })
            .await
            .map_err(|error| invalid(error.to_string()))?
        {
            ComposerReply::ImageParts(parts) => Ok(parts),
            _ => Err(invalid("The message image response was invalid.")),
        }
    }

    pub async fn chat_message_images(
        &self,
        chat_id: &str,
        context: ChatImageContext,
    ) -> Result<Vec<ChatImage>, SessionError> {
        let parts = self.image_parts(chat_id, context.clone()).await?;
        Ok(parts
            .iter()
            .enumerate()
            .map(|(index, _)| ChatImage {
                source: ChatImageSource::Message {
                    context: context.clone(),
                    index: index as u32,
                },
                name: format!("Attached image {}", index + 1),
            })
            .collect())
    }

    pub async fn chat_image_preview(
        &self,
        chat_id: &str,
        source: ChatImageSource,
        full: bool,
    ) -> Result<ChatImagePreview, SessionError> {
        let folder = self.composer_folder(chat_id).await?;
        let permit = DECODES
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| invalid("Image decoding stopped."))?;
        let blob = match source {
            ChatImageSource::Stored { sha256 } => {
                if !crate::chatstore::blobs::is_sha256(&sha256) {
                    return Err(invalid("The saved image reference is invalid."));
                }
                self.deps()
                    .store
                    .composer(ComposerCommand::ResolveImage {
                        chat_id: chat_id.into(),
                        sha256,
                    })
                    .await
                    .map_err(|error| invalid(error.to_string()))?
                    .pop()
                    .ok_or_else(|| invalid("This image is unavailable in this chat."))?
                    .sha256
            }
            ChatImageSource::Project { path } => {
                return self
                    .deps()
                    .rt
                    .spawn_blocking(move || {
                        let _permit = permit;
                        let bytes = read_project_image(&folder, &path)?;
                        preview(&bytes, full)
                    })
                    .await
                    .map_err(|_| invalid("The project image could not be prepared."))?;
            }
            ChatImageSource::Attachment { id } => {
                if id.is_empty() || id.len() > 128 {
                    return Err(invalid("The attachment image reference is invalid."));
                }
                let item = self
                    .deps()
                    .store
                    .composer(ComposerCommand::Resolve {
                        chat_id: chat_id.into(),
                        ids: vec![id],
                    })
                    .await
                    .map_err(|error| invalid(error.to_string()))?
                    .pop()
                    .ok_or_else(|| invalid("This image is no longer available."))?;
                if !item.meta.image {
                    return Err(invalid("This attachment is not an image."));
                }
                item.sha256
            }
            ChatImageSource::Message { context, index } => {
                if index >= 8 {
                    return Err(invalid("The message image reference is invalid."));
                }
                let parts = self.image_parts(chat_id, context).await?;
                match parts.get(index as usize) {
                    Some(PromptPart::Image { blob, .. }) => blob.clone(),
                    _ => return Err(invalid("This message image is no longer available.")),
                }
            }
        };
        let dir = self.deps().store.attachment_dir().to_path_buf();
        self.deps()
            .rt
            .spawn_blocking(move || {
                let _permit = permit;
                let bytes = ghost_drivers::attachments::read(&dir, &blob, MAX_SOURCE_BYTES)
                    .map_err(invalid)?;
                preview(&bytes, full)
            })
            .await
            .map_err(|_| invalid("The image preview could not be prepared."))?
    }
}

fn preview(bytes: &[u8], full: bool) -> Result<ChatImagePreview, SessionError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(invalid("The image exceeds 12 MiB."));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid("This image format is not supported."))?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| invalid("This image is damaged, too large, or unsupported."))?;
    let edge = if full { 2048 } else { 320 };
    let resized = if decoded.width() > edge || decoded.height() > edge {
        decoded.resize(edge, edge, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let (width, height) = (resized.width(), resized.height());
    let mut encoded = Cursor::new(Vec::new());
    resized
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|_| invalid("The image preview could not be encoded."))?;
    let bytes = encoded.into_inner();
    let bound = if full { 4 * 1024 * 1024 } else { 512 * 1024 };
    if bytes.len() > bound {
        return Err(invalid("The image preview is too large."));
    }
    Ok(ChatImagePreview {
        png_base64: STANDARD.encode(bytes),
        width,
        height,
    })
}

fn read_project_image(folder: &Path, source: &str) -> Result<Vec<u8>, SessionError> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    if source.is_empty() || source.len() > 4096 || source.contains('\0') {
        return Err(invalid("The image path is invalid."));
    }
    let root = crate::fsutil::plain_path(
        &folder
            .canonicalize()
            .map_err(|_| invalid("The chat folder is unavailable."))?,
    );
    let source = crate::fsutil::plain_path(Path::new(source));
    let source = source.as_path();
    let relative = if source.is_absolute() {
        source
            .strip_prefix(&root)
            .or_else(|_| source.strip_prefix(folder))
            .map_err(|_| invalid("Image previews must stay inside this chat's folder."))?
    } else {
        source
    };
    let relative: PathBuf = relative
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect();
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || relative
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case(".git"))
        || super::composer::restricted(&relative)
    {
        return Err(invalid("Choose an image inside this chat's folder."));
    }
    // Every component is opened without following links, including Windows reparse points.
    // The final read uses that handle, so a path replaced after validation cannot escape the root.
    let mut directory = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority())
        .map_err(|_| invalid("The chat folder is unavailable."))?;
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        directory = directory
            .open_dir_nofollow(part.as_os_str())
            .map_err(|_| invalid("The image's parent folder changed or is a link."))?;
    }
    let name = relative
        .file_name()
        .ok_or_else(|| invalid("Choose an image file."))?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    let file = directory
        .open_with(name, &options)
        .map_err(|_| invalid("The image is missing, is a link, or could not be opened."))?;
    let metadata = file
        .metadata()
        .map_err(|_| invalid("The image could not be inspected."))?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(invalid("Choose a regular image smaller than 12 MiB."));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("The image could not be read."))?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(invalid("The image exceeds 12 MiB."));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agent_images_normalize_persist_reopen_and_require_chat_ownership() {
        use crate::{owner::ProfileOwner, paths::DataRoot, profile::Profiles, store::Store};
        use ghost_drivers::incoming_images::AttachmentWriter;
        let dir = tempfile::tempdir().unwrap();
        let profile = Profiles::new(DataRoot::at(dir.path().join("data")))
            .open_active()
            .unwrap();
        let owner = ProfileOwner::acquire(&profile).unwrap();
        let (store, _) = Store::open(&profile).unwrap();
        let store = crate::chatstore::ChatStore::start(
            store,
            owner,
            &profile,
            "dev".into(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        store.recovered().await;
        let chat = store
            .create_chat(crate::chatstore::NewChat {
                agent_host: Default::default(),
                agent_id: "mock".into(),
                folder: dir.path().to_path_buf(),
                device_id: "dev".into(),
                now: 1,
            })
            .await
            .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(80, 40)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        let writer = StoredImages {
            store: store.clone(),
            chat_id: chat.id.clone(),
            rt: tokio::runtime::Handle::current(),
        };
        let hash = writer.write(bytes.into_inner()).await.unwrap();
        assert!(writer.write(b"not an image".to_vec()).await.is_err());
        assert!(writer.write(vec![0; MAX_SOURCE_BYTES + 1]).await.is_err());
        assert!(
            store
                .composer(ComposerCommand::ResolveImage {
                    chat_id: "other-chat".into(),
                    sha256: hash.clone()
                })
                .await
                .is_err()
        );
        let saved =
            ghost_drivers::attachments::read(store.attachment_dir(), &hash, MAX_SOURCE_BYTES)
                .unwrap();
        assert!(saved.starts_with(b"\x89PNG\r\n\x1a\n"));
        drop(writer);
        assert!(store.shutdown(std::time::Duration::from_secs(5)).await);
        drop(store);
        let owner = ProfileOwner::acquire(&profile).unwrap();
        let (store, _) = Store::open(&profile).unwrap();
        let store = crate::chatstore::ChatStore::start(
            store,
            owner,
            &profile,
            "dev".into(),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        store.recovered().await;
        let resolved = store
            .composer(ComposerCommand::ResolveImage {
                chat_id: chat.id,
                sha256: hash.clone(),
            })
            .await
            .unwrap();
        assert_eq!(resolved[0].sha256, hash);
        assert!(store.shutdown(std::time::Duration::from_secs(5)).await);
    }

    #[test]
    fn previews_are_bounded_pngs_and_reject_active_or_damaged_formats() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2400, 1200)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let small = preview(bytes.get_ref(), false).unwrap();
        assert_eq!((small.width, small.height), (320, 160));
        let png = STANDARD.decode(small.png_base64).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let large = preview(bytes.get_ref(), true).unwrap();
        assert_eq!((large.width, large.height), (2048, 1024));
        assert!(preview(b"<svg><script>alert(1)</script></svg>", false).is_err());
        assert!(preview(b"not an image", false).is_err());
    }

    #[test]
    fn project_reads_are_bounded_and_confined_to_regular_chat_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("chat");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("image.png"), "fixture").unwrap();
        std::fs::write(dir.path().join("outside.png"), "outside").unwrap();
        assert_eq!(read_project_image(&root, "image.png").unwrap(), b"fixture");
        assert_eq!(
            read_project_image(&root, "./image.png").unwrap(),
            b"fixture"
        );
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/image.png"), "nested").unwrap();
        assert_eq!(
            read_project_image(&root, "./nested/./image.png").unwrap(),
            b"nested"
        );
        assert_eq!(
            read_project_image(&root, &root.join("image.png").to_string_lossy()).unwrap(),
            b"fixture"
        );
        for path in [
            "../outside.png",
            ".git/config",
            ".env",
            "missing.png",
            "image.png/child",
        ] {
            assert!(read_project_image(&root, path).is_err(), "{path}");
        }
        assert!(
            read_project_image(&root, &dir.path().join("outside.png").to_string_lossy()).is_err()
        );
        let large = std::fs::File::create(root.join("large.png")).unwrap();
        large.set_len(MAX_SOURCE_BYTES as u64 + 1).unwrap();
        assert!(read_project_image(&root, "large.png").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_reads_refuse_symlinks_in_any_component() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("image.png"), "outside").unwrap();
        symlink(outside.path(), root.path().join("link")).unwrap();
        symlink(
            outside.path().join("image.png"),
            root.path().join("image.png"),
        )
        .unwrap();
        assert!(read_project_image(root.path(), "link/image.png").is_err());
        assert!(read_project_image(root.path(), "image.png").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn project_reads_refuse_windows_junctions() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("image.png"), "outside").unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.path().join("link"))
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture: {status:?}");
        assert!(read_project_image(root.path(), "link/image.png").is_err());
        // Remove only the known junction itself before TempDir cleanup.
        std::fs::remove_dir(root.path().join("link")).unwrap();
    }
}
