//! Identifiers for everything Ghost stores.

/// A new UUIDv7 as a lowercase, hyphenated string.
///
/// Version 7 puts the creation time in the high bits, so ids sort by creation time (good B-tree
/// locality in SQLite) while staying unique across devices without any coordination. Within one
/// process the `uuid` crate also keeps them strictly increasing. The lowercase form satisfies the
/// profile id rules, so a UUIDv7 can name a profile directory as is.
pub fn new_id() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_lowercase_hyphenated_uuid_v7() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id, id.to_lowercase());
        let parsed = uuid::Uuid::parse_str(&id).expect("valid uuid");
        assert_eq!(parsed.get_version_num(), 7);
        assert!(crate::paths::validate_profile_id(&id).is_ok());
    }

    #[test]
    fn ids_sort_in_creation_order_and_never_repeat() {
        let ids: Vec<String> = (0..1000).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, ids);
    }
}
