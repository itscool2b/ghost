//! Byte-bounded Git input avoids a process per small group of checkpoint files.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;

use futures::{StreamExt, TryStreamExt, stream};

use super::scan::{SavedFile, Scan};
use super::{CheckpointError, Operation, Result, cache, git, scan};

const WRITE_CONCURRENCY: usize = 4;
const OBJECTS_PER_BATCH: usize = 256;

struct Batch {
    input: Vec<u8>,
    records: usize,
}

fn batches(records: impl IntoIterator<Item = Result<Vec<u8>>>) -> Result<Vec<Batch>> {
    bounded_batches(records, usize::MAX)
}

fn bounded_batches(
    records: impl IntoIterator<Item = Result<Vec<u8>>>,
    max_records: usize,
) -> Result<Vec<Batch>> {
    let mut batches = Vec::new();
    let mut batch = Batch {
        input: Vec::new(),
        records: 0,
    };
    for record in records {
        let record = record?;
        if record.len() > git::INPUT_LIMIT {
            return Err(CheckpointError::Message(
                "A checkpoint Git input record exceeded its safety limit.".into(),
            ));
        }
        if batch.input.len() + record.len() > git::INPUT_LIMIT || batch.records == max_records {
            batches.push(batch);
            batch = Batch {
                input: Vec::new(),
                records: 0,
            };
        }
        batch.input.extend(record);
        batch.records += 1;
    }
    if batch.records > 0 {
        batches.push(batch);
    }
    Ok(batches)
}

fn invalid_stage() -> CheckpointError {
    CheckpointError::Message("Git returned an invalid checkpoint staging index.".into())
}

fn copy_names(input: &[u8]) -> Result<Vec<String>> {
    if input.len() > git::INPUT_LIMIT {
        return Err(invalid_stage());
    }
    let input = input.strip_suffix(&[0]).ok_or_else(invalid_stage)?;
    let names: Vec<_> = input
        .split(|byte| *byte == 0)
        .take(OBJECTS_PER_BATCH + 1)
        .map(|name| {
            if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
                return Err(invalid_stage());
            }
            String::from_utf8(name.to_vec()).map_err(|_| invalid_stage())
        })
        .collect::<Result<_>>()?;
    if names.len() > OBJECTS_PER_BATCH {
        return Err(invalid_stage());
    }
    Ok(names)
}

/// Stage output is sorted by Git's path order, not by the input order. Match every exact numeric
/// identity once and then restore the copy order; reject conflicts, links and unrelated entries.
fn stage_ids(output: &[u8], expected: &[String]) -> Result<Vec<String>> {
    if expected.is_empty()
        || expected.len() > OBJECTS_PER_BATCH
        || output.len() > expected.iter().map(|name| name.len() + 65).sum()
    {
        return Err(invalid_stage());
    }
    let output = output.strip_suffix(&[0]).ok_or_else(invalid_stage)?;
    let mut ids: BTreeMap<&str, Option<String>> =
        expected.iter().map(|name| (name.as_str(), None)).collect();
    if ids.len() != expected.len() {
        return Err(invalid_stage());
    }
    for record in output.split(|byte| *byte == 0) {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(invalid_stage)?;
        let header: Vec<_> = record[..tab].split(|byte| *byte == b' ').collect();
        if header.len() != 3
            || !matches!(header[0], b"100644" | b"100755")
            || header[2] != b"0"
            || header[1].len() != 40
        {
            return Err(invalid_stage());
        }
        let name = std::str::from_utf8(&record[tab + 1..]).map_err(|_| invalid_stage())?;
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_stage());
        }
        let slot = ids.get_mut(name).ok_or_else(invalid_stage)?;
        if slot.is_some() {
            return Err(invalid_stage());
        }
        *slot = Some(git::oid(header[1])?);
    }
    expected
        .iter()
        .map(|name| {
            ids.remove(name.as_str())
                .flatten()
                .ok_or_else(invalid_stage)
        })
        .collect()
}

/// Independent object writes can overlap their durable flushes. The futures stay owned by this
/// call: cancellation or the first error drops all remaining helpers before returning. Retain
/// each batch's input position because completion order must never change the file/OID mapping.
async fn write_batches<F, Fut>(batches: Vec<Batch>, run: F) -> Result<Vec<String>>
where
    F: Fn(usize, Batch) -> Fut,
    Fut: Future<Output = Result<Vec<String>>>,
{
    let completed: BTreeMap<usize, Vec<String>> =
        stream::iter(batches.into_iter().enumerate().map(|(index, batch)| {
            let expected = batch.records;
            let work = run(index, batch);
            async move {
                let ids = work.await?;
                if ids.len() != expected {
                    return Err(invalid_stage());
                }
                Ok((index, ids))
            }
        }))
        .buffer_unordered(WRITE_CONCURRENCY)
        .try_collect()
        .await?;
    Ok(completed.into_values().flatten().collect())
}

fn index_record(path: &str, file: &SavedFile) -> Result<Vec<u8>> {
    scan::validate(path)?;
    git::oid(file.oid.as_bytes())?;
    if !matches!(file.fingerprint.mode, 0o100644 | 0o100755) {
        return Err(CheckpointError::Message(
            "The checkpoint contains an unsupported file mode.".into(),
        ));
    }
    Ok(format!("{:o} {}\t{path}\0", file.fingerprint.mode, file.oid).into_bytes())
}

/// `retained` deliberately also supports old excluded files for `diff_stat`. Captures pass only
/// verified reusable files; callers decide which old entries belong in the new tree.
pub(super) async fn write_tree(
    operation: &Operation,
    root: &Path,
    scanned: &Scan,
    retained: Option<&BTreeMap<String, SavedFile>>,
) -> Result<(BTreeMap<String, SavedFile>, String)> {
    operation.context.check()?;
    let mut files = retained.cloned().unwrap_or_default();
    if let Some((_, first_copy)) = scanned.copies.first() {
        let directory = first_copy.parent().ok_or_else(|| {
            CheckpointError::Message("Checkpoint scratch directory was lost.".into())
        })?;
        if !directory.starts_with(operation.scratch.path()) {
            return Err(CheckpointError::Message(
                "Checkpoint scratch files escaped the operation directory.".into(),
            ));
        }
        let batch_fsync =
            super::quarantine::prepare(&operation.repo, operation.context.clone()).await?;
        // Bound each durable object transaction as well as its input bytes. A fresh isolated
        // staging repo supplies Git's upstream batch flush protocol without reading attributes
        // or filters from either the workspace or the actual checkpoint repository.
        let inputs = bounded_batches(
            scanned.copies.iter().map(|(_, copy)| {
                let name = copy.file_name().and_then(|name| name.to_str());
                match name {
                    Some(name)
                        if copy.parent() == Some(directory)
                            && !name.is_empty()
                            && name.bytes().all(|byte| byte.is_ascii_digit()) =>
                    {
                        Ok(format!("{name}\0").into_bytes())
                    }
                    _ => Err(CheckpointError::Message(
                        "Checkpoint scratch files have an invalid identity.".into(),
                    )),
                }
            }),
            OBJECTS_PER_BATCH,
        )?;
        let staging_repo = operation.scratch.path().join("staging.git");
        operation.git.init(&staging_repo, directory).await?;
        let objects = operation.repo.join("objects");
        tracing::info!(
            batch_fsync,
            file_count = scanned.copies.len(),
            batch_count = inputs.len(),
            "Checkpoint object write strategy"
        );
        let ids = write_batches(inputs, |number, batch| {
            let staging_repo = &staging_repo;
            let objects = &objects;
            let batch_index = operation
                .scratch
                .path()
                .join(format!("staging-index-{number}"));
            async move {
                operation.context.check()?;
                let names = copy_names(&batch.input)?;
                let max_output = batch.input.len() + batch.records * 64;
                let output = operation
                    .git
                    .write_objects(
                        staging_repo,
                        directory,
                        &batch_index,
                        objects,
                        batch.input,
                        max_output,
                        batch_fsync,
                    )
                    .await?;
                stage_ids(&output, &names)
            }
        })
        .await?;
        for ((path, _), oid) in scanned.copies.iter().zip(ids) {
            let fingerprint = scanned.files.get(path).ok_or_else(|| {
                CheckpointError::Message("Checkpoint file metadata was lost.".into())
            })?;
            files.insert(
                path.clone(),
                SavedFile {
                    fingerprint: fingerprint.clone(),
                    oid,
                },
            );
        }
        // Git can accept an existing object without reading or replacing it. Verify the newly
        // written copies too, so a cold retry after cache invalidation
        // cannot publish a manifest that still points at damaged storage.
        let written = scanned
            .copies
            .iter()
            .map(|(path, _)| {
                files
                    .get(path)
                    .cloned()
                    .map(|file| (path.clone(), file))
                    .ok_or_else(|| {
                        CheckpointError::Message("Checkpoint file metadata was lost.".into())
                    })
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        if !cache::verify_blobs(operation, root, &written).await? {
            return Err(CheckpointError::Message(
                "A newly written checkpoint object is missing. No checkpoint was published.".into(),
            ));
        }
    }
    // An empty private index naturally omits deleted files. Retained files and captured files
    // are inserted together, with NUL records preserving spaces, Unicode and literal pathspecs.
    for batch in batches(files.iter().map(|(path, file)| index_record(path, file)))? {
        operation.context.check()?;
        operation
            .git
            .run_input(
                &operation.repo,
                root,
                &operation.index,
                vec!["update-index".into(), "-z".into(), "--index-info".into()],
                batch.input,
                4096,
            )
            .await?;
    }
    operation.context.check()?;
    let tree = git::oid(
        &operation
            .git
            .run(
                &operation.repo,
                root,
                &operation.index,
                vec!["write-tree".into()],
                128,
            )
            .await?,
    )?;
    Ok((files, tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoints::Checkpoints;
    use crate::checkpoints::coordination;
    use crate::paths::DataRoot;
    use crate::profile::Profiles;
    use futures::FutureExt;
    use ghost_process::{EnvResolver, EnvSource, LeftoverRegistry, ResolvedEnv, process_env};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    struct ActiveWrite(Arc<AtomicUsize>);

    impl Drop for ActiveWrite {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct WriteControls {
        releases: Vec<Option<oneshot::Sender<bool>>>,
        active: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
    }

    impl WriteControls {
        fn release(&mut self, index: usize, success: bool) {
            self.releases[index].take().unwrap().send(success).unwrap();
        }
    }

    fn controlled_writes(
        count: usize,
    ) -> (impl Future<Output = Result<Vec<String>>>, WriteControls) {
        let (releases, waits): (Vec<_>, Vec<_>) = (0..count)
            .map(|_| {
                let (release, wait) = oneshot::channel();
                (Some(release), Some(wait))
            })
            .unzip();
        let active = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let controls = WriteControls {
            releases,
            active: Arc::clone(&active),
            started: Arc::clone(&started),
        };
        let waits = Mutex::new(waits);
        let inputs = (0..count)
            .map(|index| Batch {
                input: vec![index as u8],
                records: 1,
            })
            .collect();
        let work = write_batches(inputs, move |_, batch| {
            let index = batch.input[0] as usize;
            let wait = waits.lock().unwrap()[index].take().unwrap();
            let active = Arc::clone(&active);
            let started = Arc::clone(&started);
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                let _active = ActiveWrite(active);
                assert!(current <= WRITE_CONCURRENCY, "too many concurrent writes");
                started.fetch_add(1, Ordering::SeqCst);
                if wait.await.unwrap() {
                    Ok(vec![format!("{:040x}", index + 1)])
                } else {
                    Err(CheckpointError::Message("injected write failure".into()))
                }
            }
        });
        (work, controls)
    }

    #[tokio::test]
    async fn concurrent_writes_preserve_input_order_and_admit_at_most_four_helpers() {
        let (work, mut controls) = controlled_writes(6);
        let mut work = Box::pin(work);
        assert!(work.as_mut().now_or_never().is_none());
        assert_eq!(controls.active.load(Ordering::SeqCst), 4);
        assert_eq!(controls.started.load(Ordering::SeqCst), 4);

        // Later batches finish first. Each freed slot admits just one waiting helper.
        controls.release(3, true);
        assert!(work.as_mut().now_or_never().is_none());
        assert_eq!(controls.active.load(Ordering::SeqCst), 4);
        assert_eq!(controls.started.load(Ordering::SeqCst), 5);
        controls.release(4, true);
        assert!(work.as_mut().now_or_never().is_none());
        assert_eq!(controls.active.load(Ordering::SeqCst), 4);
        assert_eq!(controls.started.load(Ordering::SeqCst), 6);
        for index in [1, 5, 2, 0] {
            controls.release(index, true);
        }
        let ids = work.await.unwrap();
        assert_eq!(
            ids,
            (1..=6)
                .map(|index| format!("{index:040x}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(controls.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_or_cancelled_writing_drops_every_live_helper_and_starts_no_later_batch() {
        for fail in [false, true] {
            let (work, mut controls) = controlled_writes(6);
            let mut work = Box::pin(work);
            assert!(work.as_mut().now_or_never().is_none());
            assert_eq!(controls.started.load(Ordering::SeqCst), 4);
            if fail {
                controls.release(2, false);
                assert!(work.await.is_err());
            } else {
                drop(work);
            }
            assert_eq!(controls.active.load(Ordering::SeqCst), 0);
            assert_eq!(controls.started.load(Ordering::SeqCst), 4);
            for release in controls.releases.into_iter().flatten() {
                assert!(
                    release.is_closed(),
                    "no helper remains detached after return"
                );
            }
        }
    }

    #[test]
    fn cold_write_batches_also_bound_fsync_work_per_helper() {
        let result =
            bounded_batches((0..513).map(|_| Ok(b"0\n".to_vec())), OBJECTS_PER_BATCH).unwrap();
        assert_eq!(
            result.iter().map(|batch| batch.records).collect::<Vec<_>>(),
            [256, 256, 1]
        );
    }

    #[test]
    fn input_batches_respect_the_byte_limit_without_splitting_records() {
        let first = vec![b'a'; git::INPUT_LIMIT - 5];
        let unicode = "é\t\0".as_bytes().to_vec();
        let last = vec![b'z'; 2];
        let result = batches([Ok(first.clone()), Ok(unicode.clone()), Ok(last.clone())]).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].records, 2);
        assert_eq!(result[1].records, 1);
        assert_eq!(result[0].input, [first, unicode].concat());
        assert_eq!(result[1].input, last);
        assert!(
            result
                .iter()
                .all(|batch| batch.input.len() <= git::INPUT_LIMIT)
        );
        assert!(batches([Ok(vec![0; git::INPUT_LIMIT + 1])]).is_err());
        assert!(batches(std::iter::empty()).unwrap().is_empty());
    }

    #[test]
    fn stage_output_maps_numeric_identity_in_input_order_and_rejects_invalid_records() {
        let first = "0123456789abcdef0123456789abcdef01234567";
        let second = "abcdef0123456789abcdef0123456789abcdef01";
        let expected = vec!["2".into(), "10".into()];
        let output = [
            format!("100644 {second} 0\t10\0"),
            format!("100755 {first} 0\t2\0"),
        ]
        .concat();
        assert_eq!(
            stage_ids(output.as_bytes(), &expected).unwrap(),
            [first, second]
        );
        for bad in [
            format!("100644 {first} 0\t2\0"),         // Missing file.
            format!("{output}100644 {first} 0\t2\0"), // Duplicate file.
            output.replace("\t10\0", "\t11\0"),       // Unrequested file.
            output.replace(" 0\t", " 1\t"),           // Conflict stage.
            output.replace("100755", "120000"),       // Symlink.
            output.replace(first, "not-an-object-id"),
            output.trim_end_matches('\0').to_owned(),
            output.replace("\t2\0", "\t../2\0"),
            format!("{output}\0"),
        ] {
            assert!(stage_ids(bad.as_bytes(), &expected).is_err());
        }
        assert!(stage_ids(output.as_bytes(), &["2".into(), "2".into()]).is_err());
        assert_eq!(copy_names(b"2\x0010\0").unwrap(), expected);
        for bad in [b"2\n".as_slice(), b"2\0\0", b"../2\0", b"2\0a\0", b"\0"] {
            assert!(copy_names(bad).is_err());
        }
        assert!(copy_names(&vec![0; git::INPUT_LIMIT + 1]).is_err());
        assert!(copy_names(&b"0\0".repeat(OBJECTS_PER_BATCH + 1)).is_err());
    }

    #[tokio::test]
    async fn concurrent_writer_preserves_retained_bytes_and_file_mapping_across_batches() {
        let temp = tempfile::tempdir().unwrap();
        let profile = Profiles::new(DataRoot::at(temp.path().join("data")))
            .open_active()
            .unwrap();
        let work = temp.path().join("work with spaces");
        std::fs::create_dir(&work).unwrap();
        let work = std::fs::canonicalize(work).unwrap();
        let env = EnvResolver::fixed(ResolvedEnv {
            vars: process_env(),
            source: EnvSource::Fallback {
                reason: "batched checkpoint fixture".into(),
            },
            warnings: vec![],
            resolved_at_ms: 0,
        });
        let service = Checkpoints::new(&profile, env, LeftoverRegistry::disabled());
        let folder = crate::ids::new_id();
        let owner = coordination::acquire(&service.root, &folder, &work)
            .await
            .unwrap();
        let operation = service
            .operation(&folder, &work, true, owner.context())
            .await
            .unwrap();
        // Both repositories advertise a required failing filter and forced text conversion.
        // Private staging must consult neither configuration nor attributes, and the user's
        // index must remain byte-for-byte untouched even though it is not a valid Git index.
        std::fs::create_dir_all(operation.repo.join("info")).unwrap();
        std::fs::write(
            operation.repo.join("info/attributes"),
            "* filter=poison text eol=lf\n",
        )
        .unwrap();
        let config = std::fs::read_to_string(operation.repo.join("config")).unwrap();
        std::fs::write(
            operation.repo.join("config"),
            format!("{config}\n[filter \"poison\"]\n\tclean = false\n\trequired = true\n"),
        )
        .unwrap();
        std::fs::create_dir(work.join(".git")).unwrap();
        std::fs::write(
            work.join(".git/config"),
            "[filter \"poison\"]\n\tclean = false\n\trequired = true\n",
        )
        .unwrap();
        std::fs::write(work.join(".gitattributes"), "* filter=poison text eol=lf\n").unwrap();
        let user_index = b"untouched user index fixture\0";
        std::fs::write(work.join(".git/index"), user_index).unwrap();
        let copies = operation.scratch.path().join("original copies");
        std::fs::create_dir(&copies).unwrap();
        let original = b"original\r\n\0\xff";
        std::fs::write(work.join("retained.txt"), original).unwrap();
        let scanned = scan::collect(&work, &["retained.txt".into()], Some(&copies)).unwrap();
        let (retained, _) = write_tree(&operation, &work, &scanned, None).await.unwrap();

        let operation = service
            .operation(&folder, &work, false, owner.context())
            .await
            .unwrap();
        let copies = operation.scratch.path().join("fresh copies");
        std::fs::create_dir(&copies).unwrap();
        let paths: Vec<_> = (0..1025).map(|i| format!("file {i} é.txt")).collect();
        for (index, path) in paths.iter().enumerate() {
            let contents = if index % OBJECTS_PER_BATCH == 0 {
                b"shared object across concurrent batches\r\n\0".as_slice()
            } else {
                path.as_bytes()
            };
            std::fs::write(work.join(path), contents).unwrap();
        }
        let scanned = scan::collect(&work, &paths, Some(&copies)).unwrap();
        let before = operation.git.helper_count();
        let (files, tree) = write_tree(&operation, &work, &scanned, Some(&retained))
            .await
            .unwrap();
        assert_eq!(operation.git.helper_count() - before, 14);
        assert_eq!(files.len(), 1026);
        for path in &paths {
            assert_eq!(files[path].fingerprint, scanned.files[path]);
        }
        let shared_oid = &files[&paths[0]].oid;
        for index in (0..paths.len()).step_by(OBJECTS_PER_BATCH) {
            assert_eq!(&files[&paths[index]].oid, shared_oid);
        }
        let listed = operation
            .git
            .run(
                &operation.repo,
                &work,
                &operation.index,
                vec![
                    "ls-tree".into(),
                    "-rz".into(),
                    "--name-only".into(),
                    tree.clone().into(),
                ],
                64 * 1024,
            )
            .await
            .unwrap();
        assert_eq!(listed.iter().filter(|byte| **byte == 0).count(), 1026);
        let restored = service
            .blob(&operation, &work, files.get("retained.txt").unwrap())
            .await
            .unwrap();
        assert_eq!(restored, original);
        assert_eq!(std::fs::read(work.join(".git/index")).unwrap(), user_index);

        let before = operation.git.helper_count();
        let result = operation
            .git
            .run_input(
                &operation.repo,
                &work,
                &operation.index,
                vec!["hash-object".into(), "--stdin".into()],
                vec![0; git::INPUT_LIMIT + 1],
                128,
            )
            .await;
        assert!(
            matches!(result, Err(CheckpointError::Message(message)) if message.contains("input exceeded"))
        );
        assert_eq!(operation.git.helper_count(), before);
        let result = operation
            .git
            .run_input(
                &operation.repo,
                &work,
                &operation.index,
                vec!["hash-object".into(), "--stdin".into()],
                b"contents".to_vec(),
                1,
            )
            .await;
        assert!(
            matches!(result, Err(CheckpointError::Message(message)) if message.contains("output exceeded"))
        );

        // A freshly scanned unchanged workspace can reuse all blob IDs without changing its
        // tree or creating scratch copies. The fallback writer still preserves retained bytes.
        let operation = service
            .operation(&folder, &work, false, owner.context())
            .await
            .unwrap();
        let copies = operation.scratch.path().join("unchanged copies");
        std::fs::create_dir(&copies).unwrap();
        let all_paths: Vec<_> = files.keys().cloned().collect();
        let unchanged = scan::collect_with(
            &work,
            &all_paths,
            Some(&copies),
            Some(&files),
            Some(&owner.context()),
        )
        .unwrap();
        assert!(unchanged.copies.is_empty());
        let before = operation.git.helper_count();
        let (same_files, same_tree) = write_tree(&operation, &work, &unchanged, Some(&files))
            .await
            .unwrap();
        assert_eq!(operation.git.helper_count() - before, 2);
        assert_eq!(same_tree, tree);
        assert_eq!(same_files, files);

        // A failed batch must stop the writer before it touches the index, even if other
        // concurrent batches already succeeded in storing their immutable objects.
        let operation = service
            .operation(&folder, &work, false, owner.context())
            .await
            .unwrap();
        let copies = operation.scratch.path().join("failed copies");
        std::fs::create_dir(&copies).unwrap();
        let scanned = scan::collect(&work, &all_paths, Some(&copies)).unwrap();
        std::fs::remove_file(&scanned.copies[OBJECTS_PER_BATCH * 2].1).unwrap();
        assert!(write_tree(&operation, &work, &scanned, None).await.is_err());
        assert!(
            !operation.index.exists(),
            "no index was published after object write failure"
        );
    }
}
