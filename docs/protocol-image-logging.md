# Image payloads in protocol logs

The process boundary records native stdout before drivers normalize incoming images. Its log
writer therefore removes image payloads independently of image storage or rendering. Parsed
ACP/MCP/Pi image `data`, Claude `source.data`, image resource `blob`, and Codex
`imageGeneration.result` strings become `<IMAGE_DATA>`. Image data URLs are also replaced.
MIME types, paths, dimensions, statuses, adjacent text, and existing credential redaction remain.

Oversized lines are cut before queueing. Their retained prefix may precede the image's `type` or
MIME field, so the malformed/truncated JSON fallback conservatively removes string-valued
`data`, `blob`, and `result` fields even when their discriminator is absent. Nested ordinary text
such as `result.text` remains subject to the existing bounded logging policy. This intentionally
loses some ambiguous string diagnostics instead of saving an image prefix.

This applies to new log records and recognized image shapes. It does not rewrite existing logs
or infer binary data hidden inside arbitrary prose. Saved image attachments follow their separate
profile storage policy.

Verification: all 198 `ghost-process` library tests and Clippy with warnings denied pass on Windows
x64. Regression coverage includes reordered object keys, all cut positions within payloads,
escaped key names, ordinary text/metadata, credentials, and actual on-disk log output for image
records above the four MiB parse limit.
