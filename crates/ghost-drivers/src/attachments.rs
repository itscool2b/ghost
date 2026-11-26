//! Bounded, integrity-checked reads of explicit composer imports from the private profile.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ghost_protocol::PromptPart;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{fs, io::Read, path::Path};

pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Every read passes the same byte bound and content-hash check before anything reaches a vendor
/// process.
pub fn read(dir: &Path, hash: &str, limit: usize) -> Result<Vec<u8>, String> {
    if !valid_hash(hash) {
        return Err("The attachment identifier is invalid.".into());
    }
    let path = dir.join(hash);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "The attachment is no longer available. Attach it again.")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit as u64 {
        return Err("The saved attachment is not a supported file.".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)
        .and_then(|file| file.take(limit as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| "The attachment could not be read.")?;
    if bytes.len() > limit || format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err("The saved attachment changed. Attach the original file again.".into());
    }
    Ok(bytes)
}

/// A saved image as base64 for a vendor: a PNG, or a JPEG, either one the composer decoded and
/// made again (a phone's photo stays a JPEG), whose first bytes say it is what it claims.
pub fn image(dir: Option<&Path>, hash: &str, mime: &str) -> Result<String, String> {
    let magic: &[u8] = match mime {
        "image/png" => b"\x89PNG\r\n\x1a\n",
        "image/jpeg" => &[0xFF, 0xD8, 0xFF],
        _ => return Err("The attachment must be a normalized PNG or JPEG image.".into()),
    };
    let bytes = read(
        dir.ok_or("This session cannot access saved attachments.")?,
        hash,
        MAX_IMAGE_BYTES,
    )?;
    if !bytes.starts_with(magic) {
        return Err("The saved image has an invalid format.".into());
    }
    Ok(STANDARD.encode(bytes))
}

/// Native SDK content. Text-only prompts retain their original wire representation.
pub fn claude(parts: &[PromptPart], dir: Option<&Path>) -> Result<Value, String> {
    let mut content = Vec::new();
    let mut text = String::new();
    let mut has_image = false;
    for part in parts {
        match part {
            PromptPart::Text { text: value } => {
                text.push_str(value);
                content.push(json!({"type":"text","text":value}));
            }
            PromptPart::Mention { path } => {
                let value = format!("\n@{path}");
                text.push_str(&value);
                content.push(json!({"type":"text","text":value}));
            }
            PromptPart::Image { blob, mime } => {
                has_image = true;
                content.push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":image(dir,blob,mime)?}}));
            }
        }
    }
    Ok(if has_image {
        json!(content)
    } else {
        json!(text)
    })
}

pub fn codex(parts: &[PromptPart], dir: Option<&Path>) -> Result<Vec<Value>, String> {
    parts.iter().map(|part| Ok(match part {
        PromptPart::Text { text } => json!({"type":"text","text":text}),
        PromptPart::Mention { path } => json!({"type":"mention","name":path,"path":path}),
        PromptPart::Image { blob, mime } => json!({"type":"image","url":format!("data:{mime};base64,{}",image(dir,blob,mime)?)}),
    })).collect()
}

pub fn acp(parts: &[PromptPart], dir: Option<&Path>, images: bool) -> Result<Vec<Value>, String> {
    parts.iter().map(|part| Ok(match part {
        PromptPart::Text { text } => json!({"type":"text","text":text}),
        PromptPart::Mention { path } => json!({"type":"text","text":format!("\n@{path}")}),
        PromptPart::Image { blob, mime } => {
            if !images { return Err("This agent did not advertise image support. Remove the image or choose another agent.".into()); }
            json!({"type":"image","mimeType":mime,"data":image(dir,blob,mime)?})
        }
    })).collect()
}

pub fn pi(parts: &[PromptPart], dir: Option<&Path>) -> Result<Value, String> {
    let blocks = acp(parts, dir, true)?;
    let text: Vec<&str> = blocks
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect();
    let images: Vec<&Value> = blocks
        .iter()
        .filter(|block| block["type"] == "image")
        .collect();
    let mut request = json!({"type":"prompt","message":text.join("\n")});
    if !images.is_empty() {
        request["images"] = json!(images);
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_payloads_preserve_text_and_visible_mentions() {
        let parts = vec![
            PromptPart::Text {
                text: "Review".into(),
            },
            PromptPart::Mention {
                path: "/project/main.rs".into(),
            },
        ];
        assert_eq!(
            claude(&parts, None).expect("text"),
            "Review\n@/project/main.rs"
        );
        assert_eq!(
            codex(&parts, None).expect("text")[1],
            json!({"type":"mention","name":"/project/main.rs","path":"/project/main.rs"})
        );
        assert_eq!(
            acp(&parts, None, false).expect("text")[1]["text"],
            "\n@/project/main.rs"
        );
        assert!(
            !pi(&parts, None)
                .expect("text")
                .as_object()
                .expect("object")
                .contains_key("images")
        );
    }

    /// A saved PNG or JPEG is read as what it is; a type outside the two, or bytes that do not
    /// start as the type says, are refused.
    #[test]
    fn a_saved_png_or_jpeg_is_read_as_what_it_is() {
        let dir = tempfile::tempdir().expect("dir");
        let save = |bytes: &[u8]| {
            let hash = format!("{:x}", Sha256::digest(bytes));
            fs::write(dir.path().join(&hash), bytes).expect("saved");
            hash
        };
        let png = save(b"\x89PNG\r\n\x1a\nrest");
        let jpeg = save(&[0xFF, 0xD8, 0xFF, 0xE0, 1, 2]);
        assert_eq!(
            image(Some(dir.path()), &png, "image/png").expect("png"),
            STANDARD.encode(b"\x89PNG\r\n\x1a\nrest")
        );
        assert_eq!(
            image(Some(dir.path()), &jpeg, "image/jpeg").expect("jpeg"),
            STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 1, 2])
        );
        assert!(image(Some(dir.path()), &jpeg, "image/png").is_err());
        assert!(image(Some(dir.path()), &png, "image/jpeg").is_err());
        assert!(image(Some(dir.path()), &png, "image/webp").is_err());
    }

    #[test]
    fn unsupported_missing_or_traversing_images_are_explicit_errors() {
        let parts = [PromptPart::Image {
            blob: "../secret".into(),
            mime: "image/png".into(),
        }];
        assert!(
            acp(&parts, None, false)
                .expect_err("unsupported")
                .contains("advertise")
        );
        assert!(claude(&parts, None).is_err());
        assert!(codex(&parts, None).is_err());
        assert!(pi(&parts, None).is_err());
        assert!(read(Path::new("."), "../secret", 100).is_err());
    }
}
