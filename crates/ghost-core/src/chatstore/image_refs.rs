//! Canonical private image references embedded in saved Markdown or raw event JSON.
//! Finding a reference never grants access: callers must separately prove chat ownership.

pub(crate) fn hashes(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("aura-image:")
        .filter_map(move |(offset, _)| {
            let tail = &text[offset + "aura-image:".len()..];
            let hash = tail.get(..64)?;
            (hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && tail.as_bytes().get(64).is_none_or(|byte| {
                    !byte.is_ascii_alphanumeric()
                        && !matches!(*byte, b'_' | b'-' | b'/' | b'?' | b'#')
                }))
            .then_some(hash)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_refs_survive_markdown_and_raw_json_but_near_matches_do_not() {
        let a = "a".repeat(64);
        let b = "0123456789abcdef".repeat(4);
        let text = format!(r#"![Agent image](<aura-image:{a}>) {{"raw_text":"aura-image:{b}"}}"#);
        assert_eq!(
            hashes(&text).collect::<Vec<_>>(),
            vec![a.as_str(), b.as_str()]
        );
        for text in [
            format!("aura-image:{}", "A".repeat(64)),
            format!("aura-image:{}", "a".repeat(63)),
            format!("aura-image:{a}f"),
            format!("aura-image:{a}/path"),
            format!("aura-image:{a}?query"),
            format!("AURA-IMAGE:{a}"),
            format!("aura-image:{}é", "a".repeat(63)),
        ] {
            assert_eq!(hashes(&text).count(), 0, "{text}");
        }
    }
}
