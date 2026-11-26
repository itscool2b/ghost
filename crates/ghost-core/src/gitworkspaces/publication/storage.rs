use super::*;
#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    records: Vec<Record>,
}

impl GitWorkspaces {
    pub(super) fn read_publications(&self) -> Result<Vec<Record>> {
        let path = self.root.join("publications.json");
        let bytes = match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
            Ok(meta)
                if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 1024 * 1024 =>
            {
                return Err(invalid(
                    "Publication recovery has an unsupported format. Existing records were preserved.",
                ));
            }
            Ok(_) => std::fs::read(path)?,
        };
        let journal:Journal=serde_json::from_slice(&bytes).map_err(|_|invalid("Publication recovery is unreadable or belongs to a newer app. Existing records were preserved."))?;
        if journal.version != 1 || journal.records.len() > 256 {
            return Err(invalid(
                "Publication recovery has an unsupported version or size.",
            ));
        }
        let mut folders = std::collections::HashSet::new();
        for record in &journal.records {
            let (owner, name) = record
                .repository
                .split_once('/')
                .ok_or_else(|| invalid("A saved publication target is invalid."))?;
            validate_owner(owner)?;
            validate_name(name, 100)?;
            if !folders.insert(&record.folder_id)
                || uuid::Uuid::parse_str(&record.id).is_err()
                || !oid(&record.head)
                || record.folder_id.is_empty()
                || record.folder_id.len() > 128
                || record.path.len() > 32768
                || !Path::new(&record.path).is_absolute()
                || record.branch.is_empty()
                || record.branch.len() > 1024
                || record.branch.chars().any(char::is_control)
            {
                return Err(invalid("A saved publication identity is invalid."));
            }
        }
        Ok(journal.records)
    }
    /// Saves `record` into the journal as it is now, so another folder's publication saved
    /// meanwhile is kept, and leaves that journal in `records`.
    pub(super) fn save_publication(
        &self,
        records: &mut Vec<Record>,
        record: &Record,
    ) -> Result<()> {
        let _records = self.records_lock();
        let mut current = self.read_publications()?;
        if let Some(previous) = current
            .iter_mut()
            .find(|entry| entry.folder_id == record.folder_id)
        {
            *previous = record.clone();
        } else {
            current.push(record.clone());
        }
        let journal = Journal {
            version: 1,
            records: current,
        };
        let bytes = serde_json::to_vec(&journal)
            .map_err(|_| invalid("Publication recovery could not be encoded."))?;
        if bytes.len() > 1024 * 1024 || journal.records.len() > 256 {
            return Err(invalid("Publication recovery exceeds its limit."));
        }
        crate::fsutil::write_json_atomic(&self.root.join("publications.json"), &journal)?;
        *records = journal.records;
        Ok(())
    }
}
