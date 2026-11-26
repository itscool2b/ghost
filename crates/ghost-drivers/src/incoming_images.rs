//! Persist native image payloads before protocol mapping. The stdout adapter keeps its pending
//! future across select cancellations, so stop commands remain responsive during a storage read.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{FutureExt, future::BoxFuture};
use ghost_process::{LineSource, StdoutItem};
use serde_json::{Value, json};
use std::sync::Arc;

pub trait AttachmentWriter: std::fmt::Debug + Send + Sync {
    /// The owner normalizes the bytes and writes into this session's private chat attachment store.
    fn write(&self, bytes: Vec<u8>) -> BoxFuture<'_, Result<String, String>>;
}

pub(crate) struct ImageLineSource {
    source: Option<LineSource>,
    writer: Option<Arc<dyn AttachmentWriter>>,
    pending: Option<BoxFuture<'static, (LineSource, Option<StdoutItem>)>>,
}

impl ImageLineSource {
    pub fn new(source: LineSource, writer: Option<Arc<dyn AttachmentWriter>>) -> Self {
        Self {
            source: Some(source),
            writer,
            pending: None,
        }
    }

    pub async fn next(&mut self) -> Option<StdoutItem> {
        if self.pending.is_none() {
            let mut source = self.source.take()?;
            let writer = self.writer.clone();
            self.pending = Some(
                async move {
                    let item = match source.next().await {
                        Some(StdoutItem::Line(line)) => Some(StdoutItem::Line(
                            prepare_line(line, writer.as_deref()).await,
                        )),
                        item => item,
                    };
                    (source, item)
                }
                .boxed(),
            );
        }
        let (source, item) = self.pending.as_mut()?.await;
        self.source = Some(source);
        self.pending = None;
        item
    }
}

const MAX_ENCODED: usize = 16 * 1024 * 1024;
const MAX_IMAGES: usize = 8;
struct PendingImage {
    pointer: String,
    data: String,
    generation: bool,
}

async fn prepare_line(line: String, writer: Option<&dyn AttachmentWriter>) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(&line) else {
        return line;
    };
    let mut jobs = Vec::new();
    let mut budget = MAX_ENCODED;
    if !extract(&mut value, "", &mut jobs, &mut budget) {
        return line;
    }
    for job in jobs {
        let reference = async {
            let writer = writer.ok_or("Image storage is unavailable.")?;
            let bytes = STANDARD
                .decode(&job.data)
                .map_err(|_| "The image data is malformed.")?;
            if bytes.len() > 12 * 1024 * 1024 {
                return Err("The image is too large.");
            }
            let id = writer
                .write(bytes)
                .await
                .map_err(|_| "The image could not be saved.")?;
            // Stable normalized content identity. Reads still require this chat's ownership row.
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("The saved image reference is invalid.");
            }
            Ok(format!("aura-image:{id}"))
        };
        let reference = tokio::time::timeout(std::time::Duration::from_secs(5), reference).await;
        if let Some(node) = value.pointer_mut(&job.pointer) {
            let reference = match reference {
                Ok(Ok(path)) => Some(path),
                _ => None,
            };
            if job.generation {
                if let Some(reference) = reference {
                    node["savedPath"] = Value::String(reference);
                }
            } else {
                *node = json!({"type":"text","text": reference.map(|path| format!("\n\n{}\n\n", crate::image_reference("Agent image", &path))).unwrap_or_else(|| "[Image unavailable]".into())});
            }
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
}

/// Replaces every known binary image block, even after the storage budget is exhausted. No raw
/// image bytes remain for a mapper to copy into content or raw_input. Unknown content is untouched.
fn extract(
    node: &mut Value,
    pointer: &str,
    jobs: &mut Vec<PendingImage>,
    budget: &mut usize,
) -> bool {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
    let generation = kind == "imageGeneration";
    let data_uri = |value: &Value| {
        value.as_str().is_some_and(|text| {
            text.get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        })
    };
    let block = kind == "image"
        || (kind == "resource"
            && (node["resource"]["mimeType"]
                .as_str()
                .is_some_and(|mime| mime.starts_with("image/"))
                || data_uri(&node["resource"]["uri"])));
    if generation || block {
        let mut mime = node
            .get("mimeType")
            .or_else(|| node.pointer("/source/media_type"))
            .or_else(|| node.pointer("/resource/mimeType"))
            .and_then(Value::as_str)
            .unwrap_or("image/png")
            .to_owned();
        let data = if generation {
            let result = node["result"].take();
            if data_uri(&node["savedPath"]) {
                node["savedPath"].take()
            } else {
                if result.as_str().is_none_or(str::is_empty) {
                    return true;
                }
                result
            }
        } else if node.get("data").is_some() {
            node["data"].take()
        } else if node.pointer("/source/data").is_some() {
            node["source"]["data"].take()
        } else if node.pointer("/resource/blob").is_some() {
            node["resource"]["blob"].take()
        } else if let Some(pointer) = ["/uri", "/url", "/source/url", "/resource/uri"]
            .into_iter()
            .find(|pointer| node.pointer(pointer).is_some_and(data_uri))
        {
            node.pointer_mut(pointer)
                .map(Value::take)
                .unwrap_or(Value::Null)
        } else {
            return false;
        };
        if !generation {
            *node = json!({"type":"text","text":"[Image unavailable]"});
        }
        let Value::String(mut data) = data else {
            return true;
        };
        if data
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        {
            let Some((header, payload)) = data.split_once(',') else {
                return true;
            };
            if header.len() > 128 || !header.ends_with(";base64") {
                return true;
            }
            mime = header[5..header.len() - ";base64".len()].to_ascii_lowercase();
            data = payload.to_owned();
        }
        if !matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) || data.len() > *budget
            || jobs.len() >= MAX_IMAGES
        {
            return true;
        }
        *budget -= data.len();
        jobs.push(PendingImage {
            pointer: pointer.into(),
            data,
            generation,
        });
        return true;
    }
    let mut changed = false;
    match node {
        Value::Array(nodes) => {
            for (index, value) in nodes.iter_mut().enumerate() {
                changed |= extract(value, &format!("{pointer}/{index}"), jobs, budget);
            }
        }
        Value::Object(nodes) => {
            for (key, value) in nodes {
                changed |= extract(
                    value,
                    &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                    jobs,
                    budget,
                );
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Writer(Mutex<Vec<Vec<u8>>>);
    impl AttachmentWriter for Writer {
        fn write(&self, bytes: Vec<u8>) -> BoxFuture<'_, Result<String, String>> {
            async move {
                self.0.lock().unwrap().push(bytes);
                Ok("ab".repeat(32))
            }
            .boxed()
        }
    }

    #[tokio::test]
    async fn image_blocks_are_persisted_before_mapping_without_raw_payloads() {
        let writer = Writer::default();
        for value in [
            json!({"type":"image","mimeType":"image/png","data":"aW1hZ2U="}),
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}),
            json!({"type":"resource","resource":{"mimeType":"image/png","blob":"aW1hZ2U=","uri":"fixture"}}),
            json!({"type":"imageGeneration","result":"aW1hZ2U=","savedPath":null}),
            json!({"type":"image","uri":"data:image/png;base64,aW1hZ2U="}),
            json!({"type":"image","source":{"type":"url","url":"data:image/png;base64,aW1hZ2U="}}),
            json!({"type":"resource","resource":{"mimeType":"image/png","uri":"data:image/png;base64,aW1hZ2U="}}),
        ] {
            let prepared = prepare_line(
                json!({"item":{"result":{"content":[value]}}}).to_string(),
                Some(&writer),
            )
            .await;
            assert!(!prepared.contains("aW1hZ2U="));
            assert!(prepared.contains(&format!("aura-image:{}", "ab".repeat(32))));
        }
        assert_eq!(*writer.0.lock().unwrap(), vec![b"image".to_vec(); 7]);
        let local = prepare_line(
            json!({"type":"imageGeneration","result":"aW1hZ2U=","savedPath":"out.png"}).to_string(),
            Some(&writer),
        )
        .await;
        assert!(!local.contains("aW1hZ2U="));
        assert!(local.contains("aura-image:"));
        assert_eq!(writer.0.lock().unwrap().len(), 8);
        let path_only = prepare_line(
            json!({"type":"imageGeneration","result":"","savedPath":"out.png"}).to_string(),
            Some(&writer),
        )
        .await;
        assert!(path_only.contains("out.png"));
        assert_eq!(writer.0.lock().unwrap().len(), 8);
    }

    #[tokio::test]
    async fn malformed_oversized_and_excess_images_never_reach_storage_or_transcript() {
        let writer = Writer::default();
        for data in ["not base64!".to_owned(), "a".repeat(MAX_ENCODED + 1)] {
            let output = prepare_line(
                json!({"type":"image","data":data}).to_string(),
                Some(&writer),
            )
            .await;
            assert_eq!(
                serde_json::from_str::<Value>(&output).unwrap(),
                json!({"type":"text","text":"[Image unavailable]"})
            );
        }
        assert!(writer.0.lock().unwrap().is_empty());
        let images = vec![json!({"type":"image","data":"aW1hZ2U="}); 10];
        let output = prepare_line(json!({"content":images}).to_string(), Some(&writer)).await;
        assert!(!output.contains("aW1hZ2U="));
        assert_eq!(writer.0.lock().unwrap().len(), MAX_IMAGES);
        assert_eq!(output.matches("Image unavailable").count(), 2);
    }

    #[tokio::test]
    async fn missing_storage_also_scrubs_payloads_before_native_mapping() {
        let line = json!({"item":{"type":"mcpToolCall","id":"tool","result":{"content":[{"type":"image","mimeType":"image/png","data":"aW1hZ2U="}]}}}).to_string();
        let output = prepare_line(line, None).await;
        assert!(!output.contains("aW1hZ2U="));
        assert!(output.contains("Image unavailable"));
    }

    #[derive(Debug)]
    struct SlowWriter(Arc<std::sync::atomic::AtomicUsize>);
    impl AttachmentWriter for SlowWriter {
        fn write(&self, _: Vec<u8>) -> BoxFuture<'_, Result<String, String>> {
            struct Pending(Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Pending {
                fn drop(&mut self) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            async move {
                let _pending = Pending(self.0.clone());
                std::future::pending::<()>().await;
                unreachable!()
            }
            .boxed()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_and_cancellation_drop_pending_storage_without_publishing_a_reference() {
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writer = SlowWriter(dropped.clone());
        let line = json!({"type":"image","data":"aW1hZ2U="}).to_string();
        let output = prepare_line(line.clone(), Some(&writer)).await;
        assert!(!output.contains("aura-image:"));
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut pending = Box::pin(prepare_line(line, Some(&writer)));
        assert!(futures::poll!(&mut pending).is_pending());
        drop(pending);
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
