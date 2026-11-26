//! In-memory hints only. Current bytes and private Git objects are verified on every reuse.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::scan::{SavedFile, Scan};
use super::{CheckpointError, Operation, Result, git, scan};

const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const TREE_LIMIT: usize = 20_000;
// Loose-object opens also cost time when the contents are tiny. Bound that work independently
// of stdin/stdout bytes so one verification helper does not inherit a whole 20,000-file scan.
const OBJECTS_PER_HELPER: usize = 4_096;

/// OID to exact expected Git tree bytes. This includes every subtree independently, so a missing
/// parent cannot hide corruption in another existing tree during a cold-fallback decision.
pub(super) type TreeInventory = BTreeMap<String, Vec<u8>>;

#[derive(Clone)]
pub(super) struct Entry {
    folder: String,
    root: PathBuf,
    pub(super) files: BTreeMap<String, SavedFile>,
    pub(super) tree: String,
    pub(super) tree_ids: Arc<TreeInventory>,
}

#[derive(Default)]
pub(super) struct Cache(VecDeque<Arc<Entry>>);

impl Cache {
    pub(super) fn take(&mut self, folder: &str, root: &Path) -> Option<Arc<Entry>> {
        let index = self
            .0
            .iter()
            .position(|entry| entry.folder == folder && entry.root == root)?;
        self.0.remove(index)
    }

    pub(super) fn publish(
        &mut self,
        folder: &str,
        root: &Path,
        files: BTreeMap<String, SavedFile>,
        tree: String,
        tree_ids: Arc<TreeInventory>,
    ) {
        self.take(folder, root);
        self.0.push_front(Arc::new(Entry {
            folder: folder.into(),
            root: root.into(),
            files,
            tree,
            tree_ids,
        }));
        self.0.truncate(8);
    }
}

pub(super) fn retained(previous: &Entry, scanned: &Scan) -> BTreeMap<String, SavedFile> {
    scanned
        .files
        .iter()
        .filter_map(|(path, current)| {
            previous
                .files
                .get(path)
                .filter(|saved| {
                    saved.fingerprint.sha256 == current.sha256
                        && saved.fingerprint.bytes == current.bytes
                })
                .map(|saved| {
                    (
                        path.clone(),
                        SavedFile {
                            fingerprint: current.clone(),
                            oid: saved.oid.clone(),
                        },
                    )
                })
        })
        .collect()
}

fn damaged() -> CheckpointError {
    CheckpointError::Message("A cached checkpoint object is damaged. No prompt was sent.".into())
}

enum Object<'a> {
    Blob(&'a SavedFile),
    Tree(&'a str, &'a [u8]),
}

impl Object<'_> {
    fn oid(&self) -> &str {
        match self {
            Self::Blob(file) => &file.oid,
            Self::Tree(oid, _) => oid,
        }
    }
    fn size(&self) -> usize {
        match self {
            Self::Blob(file) => file.fingerprint.bytes as usize,
            Self::Tree(_, bytes) => bytes.len(),
        }
    }
    fn kind(&self) -> &str {
        match self {
            Self::Blob(_) => "blob",
            Self::Tree(_, _) => "tree",
        }
    }
    fn matches(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Blob(file) => {
                crate::chatstore::rows::hex(&Sha256::digest(bytes)) == file.fingerprint.sha256
            }
            Self::Tree(_, expected) => bytes == *expected,
        }
    }
}

/// Missing objects are recoverable cache misses. Continue through all batches so a missing object
/// cannot hide corruption in a later batch that Git might otherwise accept as already stored.
async fn verify_objects(
    operation: &Operation,
    root: &Path,
    objects: &[Object<'_>],
) -> Result<bool> {
    let mut all_present = true;
    let mut start = 0;
    while start < objects.len() {
        let mut end = start;
        let mut bytes = 0;
        while end < objects.len() {
            let cost = objects[end].size().checked_add(128).ok_or_else(damaged)?;
            if cost > OUTPUT_LIMIT {
                return Err(damaged());
            }
            if end > start
                && (bytes + cost > OUTPUT_LIMIT
                    || end - start == OBJECTS_PER_HELPER
                    || (end - start + 1) * 41 > git::INPUT_LIMIT)
            {
                break;
            }
            bytes += cost;
            end += 1;
        }
        operation.context.check()?;
        let mut input = Vec::with_capacity((end - start) * 41);
        for object in &objects[start..end] {
            git::oid(object.oid().as_bytes())?;
            input.extend_from_slice(object.oid().as_bytes());
            input.push(b'\n');
        }
        let output = operation
            .git
            .run_input(
                &operation.repo,
                root,
                &operation.index,
                vec!["cat-file".into(), "--batch".into()],
                input,
                bytes,
            )
            .await?;
        all_present &= parse_objects(&output, &objects[start..end])?;
        start = end;
    }
    Ok(all_present)
}

fn parse_objects(mut output: &[u8], objects: &[Object<'_>]) -> Result<bool> {
    let mut all_present = true;
    for object in objects {
        let end = output
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(damaged)?;
        let header = std::str::from_utf8(&output[..end]).map_err(|_| damaged())?;
        output = &output[end + 1..];
        if header == format!("{} missing", object.oid()) {
            all_present = false;
            continue;
        }
        if header != format!("{} {} {}", object.oid(), object.kind(), object.size()) {
            return Err(damaged());
        }
        let size = object.size();
        if output.len() <= size || output[size] != b'\n' || !object.matches(&output[..size]) {
            return Err(damaged());
        }
        output = &output[size + 1..];
    }
    if !output.is_empty() {
        return Err(damaged());
    }
    Ok(all_present)
}

pub(super) async fn verify_blobs(
    operation: &Operation,
    root: &Path,
    files: &BTreeMap<String, SavedFile>,
) -> Result<bool> {
    let mut unique: BTreeMap<&str, &SavedFile> = BTreeMap::new();
    for file in files.values() {
        if file.fingerprint.bytes as usize > scan::FILE_LIMIT {
            return Err(damaged());
        }
        if let Some(previous) = unique.insert(&file.oid, file)
            && (previous.fingerprint.bytes != file.fingerprint.bytes
                || previous.fingerprint.sha256 != file.fingerprint.sha256)
        {
            return Err(damaged());
        }
    }
    let objects: Vec<_> = unique.values().map(|file| Object::Blob(file)).collect();
    verify_objects(operation, root, &objects).await
}

pub(super) async fn verify_tree(
    operation: &Operation,
    root: &Path,
    previous: &Entry,
) -> Result<bool> {
    if !previous.tree_ids.contains_key(&previous.tree) {
        return Err(damaged());
    }
    let objects: Vec<_> = previous
        .tree_ids
        .iter()
        .map(|(oid, bytes)| Object::Tree(oid, bytes))
        .collect();
    verify_objects(operation, root, &objects).await
}

/// Calculate the optional inventory's bounds before asking Git to enumerate directory prefixes.
/// Very deep trees may exceed the cache budget despite fitting the ordinary capture limits.
fn inventory_bounds(
    files: &BTreeMap<String, SavedFile>,
) -> Result<Option<(BTreeSet<String>, usize)>> {
    let mut directories = BTreeSet::from([String::new()]);
    let mut output_bytes = 0usize;
    let mut tree_bytes = 128usize; // Root object's batch framing budget.
    for path in files.keys() {
        scan::validate(path)?;
        output_bytes = output_bytes.saturating_add(path.len() + 64);
        let name = path.rsplit('/').next().ok_or_else(damaged)?;
        tree_bytes = tree_bytes.saturating_add(name.len() + 28);
        if output_bytes > OUTPUT_LIMIT || tree_bytes > OUTPUT_LIMIT {
            return Ok(None);
        }
        for (end, _) in path.match_indices('/') {
            let directory = &path[..end];
            if !directories.contains(directory) {
                output_bytes = output_bytes.saturating_add(directory.len() + 64);
                let name = directory.rsplit('/').next().ok_or_else(damaged)?;
                tree_bytes = tree_bytes.saturating_add(name.len() + 27 + 128);
                if directories.len() >= TREE_LIMIT
                    || output_bytes > OUTPUT_LIMIT
                    || tree_bytes > OUTPUT_LIMIT
                {
                    return Ok(None);
                }
                directories.insert(directory.to_owned());
            }
        }
    }
    Ok(Some((directories, output_bytes.max(1))))
}

pub(super) async fn inventory(
    operation: &Operation,
    root: &Path,
    files: &BTreeMap<String, SavedFile>,
    tree: &str,
) -> Result<Option<Arc<TreeInventory>>> {
    let Some((directories, max_output)) = inventory_bounds(files)? else {
        return Ok(None);
    };
    operation.context.check()?;
    git::oid(tree.as_bytes())?;
    let output = operation
        .git
        .run(
            &operation.repo,
            root,
            &operation.index,
            vec![
                "ls-tree".into(),
                "-rtz".into(),
                "--full-tree".into(),
                tree.into(),
            ],
            max_output,
        )
        .await?;
    parse_inventory(&output, files, tree, &directories).map(|inventory| Some(Arc::new(inventory)))
}

fn parse_inventory(
    output: &[u8],
    files: &BTreeMap<String, SavedFile>,
    tree: &str,
    expected_directories: &BTreeSet<String>,
) -> Result<TreeInventory> {
    if !output.is_empty() && output.last() != Some(&0) {
        return Err(damaged());
    }
    let mut directories = BTreeMap::from([(String::new(), tree.to_owned())]);
    let mut seen_files = BTreeSet::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record).map_err(|_| damaged())?;
        let (header, path) = record.split_once('\t').ok_or_else(damaged)?;
        let mut fields = header.split(' ');
        let (Some(mode), Some(kind), Some(oid), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(damaged());
        };
        git::oid(oid.as_bytes())?;
        if kind == "tree" && mode == "040000" {
            if path.is_empty()
                || !expected_directories.contains(path)
                || directories
                    .insert(path.to_owned(), oid.to_owned())
                    .is_some()
            {
                return Err(damaged());
            }
        } else if kind == "blob" {
            let expected = files.get(path).ok_or_else(damaged)?;
            if mode != format!("{:o}", expected.fingerprint.mode)
                || oid != expected.oid
                || !seen_files.insert(path)
            {
                return Err(damaged());
            }
        } else {
            return Err(damaged());
        }
    }
    if seen_files.len() != files.len()
        || directories.len() != expected_directories.len()
        || directories
            .keys()
            .any(|path| !expected_directories.contains(path))
    {
        return Err(damaged());
    }

    // Git compares directory names as if suffixed by '/'. Build each directory's exact raw bytes
    // from the validated inventory and file map, independently of ls-tree's traversal order.
    let mut children: BTreeMap<&str, Vec<(String, u32, &str, &str)>> = BTreeMap::new();
    for (path, oid) in directories.iter().filter(|(path, _)| !path.is_empty()) {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        children
            .entry(parent)
            .or_default()
            .push((format!("{name}/"), 0o40000, name, oid));
    }
    for (path, file) in files {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        children.entry(parent).or_default().push((
            name.to_owned(),
            file.fingerprint.mode,
            name,
            &file.oid,
        ));
    }
    let mut inventory = BTreeMap::new();
    let mut total_bytes = 0usize;
    for (directory, oid) in &directories {
        let mut entries = children.remove(directory.as_str()).unwrap_or_default();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut bytes = Vec::new();
        for (_, mode, name, child_oid) in entries {
            bytes.extend_from_slice(format!("{mode:o} {name}").as_bytes());
            bytes.push(0);
            for pair in child_oid.as_bytes().as_chunks::<2>().0 {
                let pair = std::str::from_utf8(pair).map_err(|_| damaged())?;
                bytes.push(u8::from_str_radix(pair, 16).map_err(|_| damaged())?);
            }
        }
        total_bytes = total_bytes.saturating_add(bytes.len() + 128);
        if total_bytes > OUTPUT_LIMIT || bytes.len() + 128 > OUTPUT_LIMIT {
            return Err(damaged());
        }
        if let Some(previous) = inventory.insert(oid.clone(), bytes.clone())
            && previous != bytes
        {
            return Err(damaged());
        }
    }
    if !children.is_empty() {
        return Err(damaged());
    }
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(bytes: &[u8], digit: char) -> SavedFile {
        SavedFile {
            oid: digit.to_string().repeat(40),
            fingerprint: scan::Fingerprint {
                sha256: crate::chatstore::rows::hex(&Sha256::digest(bytes)),
                bytes: bytes.len() as u32,
                mode: 0o100644,
                binary: true,
            },
        }
    }

    #[test]
    fn binary_batch_framing_rejects_corruption_and_accepts_missing() {
        let bytes = b"a\0b\nc\xff";
        let saved = saved(bytes, 'a');
        let mut output = format!("{} blob {}\n", saved.oid, bytes.len()).into_bytes();
        output.extend_from_slice(bytes);
        output.push(b'\n');
        assert!(parse_objects(&output, &[Object::Blob(&saved)]).unwrap());
        output.pop();
        assert!(parse_objects(&output, &[Object::Blob(&saved)]).is_err());
        assert!(
            !parse_objects(
                format!("{} missing\n", saved.oid).as_bytes(),
                &[Object::Blob(&saved)]
            )
            .unwrap()
        );
        let wrong = format!("{} tree {}\n", saved.oid, bytes.len());
        assert!(parse_objects(wrong.as_bytes(), &[Object::Blob(&saved)]).is_err());
    }

    #[test]
    fn missing_objects_do_not_hide_later_corrupt_tree_or_blob_bodies() {
        let missing = saved(b"gone", 'a');
        let present = saved(b"correct", 'b');
        let mut output = format!("{} missing\n{} blob 7\n", missing.oid, present.oid).into_bytes();
        output.extend_from_slice(b"altered\n");
        assert!(parse_objects(&output, &[Object::Blob(&missing), Object::Blob(&present)]).is_err());
        let tree = "c".repeat(40);
        let output = format!("{} missing\n{tree} tree 4\nbad!\n", missing.oid);
        assert!(
            parse_objects(
                output.as_bytes(),
                &[Object::Blob(&missing), Object::Tree(&tree, b"good")]
            )
            .is_err()
        );
    }

    #[test]
    fn inventory_validates_maps_independent_of_order_and_encodes_directory_sorting() {
        let file = saved(b"x", 'a');
        let files = BTreeMap::from([
            ("a/file".into(), file.clone()),
            ("a.c".into(), file.clone()),
            ("a0".into(), file),
        ]);
        let root = "b".repeat(40);
        let subtree = "c".repeat(40);
        let output = format!(
            "100644 blob {}\ta0\0\
             100644 blob {}\ta/file\0\
             040000 tree {subtree}\ta\0\
             100644 blob {}\ta.c\0",
            "a".repeat(40),
            "a".repeat(40),
            "a".repeat(40),
        );
        let (directories, _) = inventory_bounds(&files).unwrap().unwrap();
        let inventory = parse_inventory(output.as_bytes(), &files, &root, &directories).unwrap();
        let expected = [
            b"100644 a.c\0".as_slice(),
            &[0xaa; 20],
            b"40000 a\0",
            &[0xcc; 20],
            b"100644 a0\0",
            &[0xaa; 20],
        ]
        .concat();
        assert_eq!(inventory[&root], expected);
        assert_eq!(
            inventory[&subtree],
            [b"100644 file\0".as_slice(), &[0xaa; 20]].concat()
        );
        assert!(parse_inventory(b"", &files, &root, &directories).is_err());
        assert!(
            parse_inventory(
                format!("{output}{output}").as_bytes(),
                &files,
                &root,
                &directories
            )
            .is_err()
        );
    }

    #[test]
    fn empty_and_oversized_inventories_are_bounded() {
        let root = "a".repeat(40);
        let files = BTreeMap::new();
        let (directories, _) = inventory_bounds(&files).unwrap().unwrap();
        let inventory = parse_inventory(b"", &files, &root, &directories).unwrap();
        assert_eq!(inventory, BTreeMap::from([(root, Vec::new())]));
        let deep = "d/".repeat(1_500);
        let files = (0..8)
            .map(|index| (format!("{index}/{deep}leaf"), saved(b"x", 'a')))
            .collect();
        assert!(inventory_bounds(&files).unwrap().is_none());
    }
}
