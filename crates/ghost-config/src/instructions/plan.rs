//! Turns a requested change into exact bytes per file, with a review that names every file that
//! is created, replaced, renamed or removed and why. Nothing is written here.

use std::collections::BTreeMap;

use super::{
    Action, Change, Content, FileKind, Prepared, Review, ReviewFile,
    agents::{AGENTS, AgentRule, Pick, RootId},
    apply::{self, Dependency, Expect, Mutation},
    imports, manifest,
    model::{Model, normalize},
    text,
};
use crate::{Error, Result, safe_fs};

const SESSIONS: &str = "Agents read instructions when a session starts. Running chats keep what they started with; new sessions read the new files.";

enum Write {
    Bytes(Vec<u8>),
    Remove,
    Rename,
}

struct Builder<'m, 'c> {
    model: &'m Model<'c>,
    writes: Vec<(usize, Write, Option<String>, Vec<String>)>,
    notes: Vec<String>,
    optional: Vec<String>,
}

impl<'m, 'c> Builder<'m, 'c> {
    fn new(model: &'m Model<'c>) -> Self {
        Self {
            model,
            writes: Vec::new(),
            notes: Vec::new(),
            optional: Vec::new(),
        }
    }

    fn written(&self, index: usize) -> Option<&Write> {
        self.writes
            .iter()
            .find(|(at, ..)| *at == index)
            .map(|(_, write, ..)| write)
    }

    fn write(&mut self, index: usize, bytes: Vec<u8>, how: Option<String>, notes: Vec<String>) {
        if let Some(entry) = self.writes.iter_mut().find(|(at, ..)| *at == index) {
            entry.1 = Write::Bytes(bytes);
            entry.2 = how.or(entry.2.take());
            entry.3.extend(notes);
            return;
        }
        let file = &self.model.files[index];
        if file.kind == FileKind::File && file.bytes.as_deref() == Some(bytes.as_slice()) {
            return;
        }
        self.writes.push((index, Write::Bytes(bytes), how, notes));
    }

    fn remove(&mut self, index: usize, notes: Vec<String>) {
        if self.written(index).is_none() && self.model.files[index].exists() {
            self.writes.push((index, Write::Remove, None, notes));
        }
    }

    fn finish(
        self,
        title: String,
        summary: String,
        manifest: Option<Option<manifest::Entry>>,
    ) -> Result<Prepared> {
        let model = self.model;
        let mut mutations = Vec::new();
        let mut files = Vec::new();
        for (index, write, how, mut notes) in self.writes {
            let file = &model.files[index];
            let replacing_link = matches!(file.kind, FileKind::Link | FileKind::HardLink);
            if !replacing_link && let Some(reason) = model.blocked(index) {
                return Err(Error::Unsupported(reason));
            }
            if replacing_link && file.read_only {
                return Err(Error::Unsupported(format!(
                    "{} is read-only. Clear its read-only setting to change it here.",
                    file.display
                )));
            }
            let before = match file.kind {
                FileKind::Link => file
                    .link_to
                    .and_then(|target| model.files[target].text.clone()),
                _ => file.text.clone(),
            };
            let (action, after, bytes, rename_from) = match write {
                Write::Bytes(bytes) => {
                    let after = text::decode(&bytes).map(|(text, _)| text);
                    if let Some(encoding) = file.encoding
                        && let Some(note) = text::encoding_note(encoding)
                    {
                        notes.push(note);
                    }
                    (
                        if file.exists() {
                            Action::Replace
                        } else {
                            Action::Create
                        },
                        after,
                        Some(bytes),
                        None,
                    )
                }
                Write::Remove => (Action::Remove, None, None, None),
                Write::Rename => (
                    Action::Rename,
                    file.text.clone(),
                    file.bytes.clone(),
                    file.disk.clone(),
                ),
            };
            if file.kind == FileKind::Link && action == Action::Replace {
                notes.push(
                    "Replaces the link with a file of its own; the file it pointed to stays."
                        .into(),
                );
            }
            if file.kind == FileKind::HardLink && action == Action::Replace {
                notes.push(
                    "Was a hard link to another instruction file; it becomes a separate file."
                        .into(),
                );
            }
            if action == Action::Create && model.project() && model.context.git.repository {
                notes.push("New and not committed yet. Commit it so new worktree chats and teammates get it.".into());
            }
            let root_id = model.roots[file.root].id;
            if action == Action::Create
                && root_id != RootId::Project
                && !model.roots[file.root].path.exists()
            {
                notes.push(format!(
                    "Creates the folder {}.",
                    model.roots[file.root].display
                ));
            }
            mutations.push(Mutation {
                root: file.root,
                path: if action == Action::Rename {
                    file.path.into()
                } else if file.exists() {
                    file.disk_path().into()
                } else {
                    file.path.into()
                },
                expect: file.expect(),
                after: if action == Action::Rename {
                    None
                } else {
                    bytes.clone()
                },
                rename_from,
            });
            let backup = matches!(action, Action::Replace | Action::Remove)
                && matches!(file.expect(), Expect::File(_));
            files.push(ReviewFile {
                key: file.key.clone(),
                path: file.display.clone(),
                action,
                before,
                after,
                how,
                notes,
                backup,
            });
        }
        let touched = mutations
            .iter()
            .map(|mutation| {
                (
                    mutation.root,
                    mutation
                        .rename_from
                        .clone()
                        .unwrap_or(mutation.path.clone()),
                )
            })
            .collect::<Vec<_>>();
        let dependencies = model
            .files
            .iter()
            .filter(|file| {
                file.exists() && !touched.contains(&(file.root, file.disk_path().to_owned()))
            })
            .filter(|file| {
                matches!(
                    file.kind,
                    FileKind::File | FileKind::HardLink | FileKind::Link | FileKind::NotText
                )
            })
            .map(|file| Dependency {
                root: file.root,
                path: file.disk_path().into(),
                expect: file.expect(),
            })
            .collect();
        // Missing files that stay missing are part of what was reviewed too.
        let missing = model
            .files
            .iter()
            .filter(|file| !file.exists() && file.kind == FileKind::Missing)
            .filter(|file| {
                !touched
                    .iter()
                    .any(|(root, path)| *root == file.root && path == file.path)
            })
            .filter(|file| model.roots[file.root].path.exists())
            .map(|file| Dependency {
                root: file.root,
                path: file.path.into(),
                expect: Expect::Missing,
            });
        let dependencies: Vec<Dependency> = dependencies_with(dependencies, missing);
        // A record alone changes nothing a person can see unless it adds or ends managed copies.
        let record_changes = match &manifest {
            None => false,
            Some(None) => model.record.is_some(),
            Some(Some(entry)) => {
                !entry.copies.is_empty()
                    && model
                        .record
                        .as_ref()
                        .is_none_or(|record| record.copies != entry.copies)
            }
        };
        // With only optional removals left, the review is where the person chooses them.
        if files.is_empty() && !record_changes && self.optional.is_empty() {
            return Err(Error::Unsupported("There is nothing to change.".into()));
        }
        let mut notes = self.notes;
        notes.push(SESSIONS.into());
        Ok(Prepared {
            review: Review {
                id: uuid::Uuid::now_v7().to_string(),
                title,
                summary,
                files,
                notes,
                optional_removals: self.optional,
            },
            roots: model.roots.iter().map(|root| root.path.clone()).collect(),
            mutations,
            dependencies,
            manifest: manifest.map(|entry| apply::MetadataIntent {
                identity: model.identity.clone(),
                before: model.record.clone(),
                after: entry,
            }),
            state_dir: model.context.state_dir.clone(),
            expires: std::time::Instant::now(),
        })
    }
}

fn dependencies_with(
    present: Vec<Dependency>,
    missing: impl Iterator<Item = Dependency>,
) -> Vec<Dependency> {
    let mut all = present;
    all.extend(missing);
    all
}

pub(crate) fn prepare(model: &Model, change: &Change) -> Result<Prepared> {
    match change {
        Change::Save { file, text } => save(model, file, text),
        Change::Universal {
            source,
            content,
            remove,
            agents,
        } => universal(model, source, content, remove, agents.as_deref()),
        Change::PerAgent => per_agent(model),
        Change::Restore { file } => restore(model, file),
        Change::Adopt { file } => adopt(model, file),
        Change::Rename { file } => rename(model, file),
    }
}

fn known(model: &Model, key: &str) -> Result<usize> {
    model
        .file(key)
        .ok_or_else(|| Error::Unsupported("That file isn't one Ghost manages here.".into()))
}

fn names(_model: &Model, ids: &[&str]) -> String {
    let names = ids
        .iter()
        .filter_map(|id| super::agents::rule(id))
        .map(|agent| agent.name)
        .collect::<Vec<_>>();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Size guidance for new text, from the agents that read this file.
fn limit_notes(model: &Model, index: usize, value: &str, bytes: usize) -> Vec<String> {
    let mut notes = Vec::new();
    let lines = text::lines(value);
    let chars = value.chars().count();
    for agent in AGENTS {
        let Some(loaded) = model.loaded(agent) else {
            continue;
        };
        let reads = loaded.files.contains(&index)
            || loaded
                .files
                .iter()
                .any(|file| model.import_of(*file) == Some(index) && agent.imports)
            || model
                .reads(agent)
                .is_some_and(|reads| model.by_path(reads.own.0, reads.own.1) == Some(index));
        if !reads {
            continue;
        }
        for limit in agent.limits {
            match *limit {
                super::agents::Limit::CombinedBytes(default) => {
                    let limit = model.context.codex_limit.unwrap_or(default);
                    if bytes > limit {
                        notes.push(format!(
                            "{} reads the first {} KiB; the end of this text is cut for it.",
                            agent.name,
                            limit / 1024
                        ));
                    }
                }
                super::agents::Limit::FileBytes(limit) if bytes > limit => notes.push(format!(
                    "{} cuts files at {limit} bytes; this text is {bytes}.",
                    agent.name
                )),
                super::agents::Limit::FileChars(limit) if chars > limit => notes.push(format!(
                    "{} may cut text over {limit} characters on smaller models.",
                    agent.name
                )),
                super::agents::Limit::SkipOver(limit) if bytes > limit => notes.push(format!(
                    "{} skips files over {} MiB.",
                    agent.name,
                    limit / 1024 / 1024
                )),
                _ => {}
            }
        }
        if let Some(recommended) = agent.recommended_lines
            && lines > recommended
        {
            notes.push(format!(
                "{} suggests under {recommended} lines; this text has {lines}.",
                agent.name
            ));
        }
    }
    notes.dedup();
    notes
}

fn save(model: &Model, key: &str, value: &str) -> Result<Prepared> {
    let mut index = known(model, key)?;
    let mut builder = Builder::new(model);
    if model.files[index].kind == FileKind::Link
        && let Some(target) = model.files[index].link_to
    {
        builder.notes.push(format!(
            "{} links to {}, so the text is saved there.",
            model.files[index].display, model.files[target].display
        ));
        index = target;
    }
    if let Some(reason) = model.blocked(index) {
        return Err(Error::Unsupported(reason));
    }
    let (mode, source) = model.mode();
    let encoding = model.files[index]
        .encoding
        .or_else(|| source.and_then(|source| model.files[source].encoding))
        .unwrap_or_default();
    let bytes = text::encode(value, encoding);
    if model.files[index].bytes.as_deref() == Some(bytes.as_slice()) {
        return Err(Error::Unsupported("The text is unchanged.".into()));
    }
    let mut notes = limit_notes(model, index, value, bytes.len());
    if value.trim().is_empty() {
        notes.push("Agents skip an empty file.".into());
    }
    builder.write(index, bytes.clone(), None, notes);
    for (twin, file) in model.files.iter().enumerate() {
        if twin != index
            && file.kind == FileKind::HardLink
            && (file.link_to == Some(index) || model.files[index].link_to == Some(twin))
        {
            builder.write(
                twin,
                bytes.clone(),
                Some(format!("Same text as {}", model.files[index].display)),
                Vec::new(),
            );
        }
    }
    let mut manifest = None;
    if mode == super::Mode::Universal && source == Some(index) {
        // Copies that still hold the old shared text follow it; a copy edited by hand stays.
        let mut copies = BTreeMap::new();
        for (copy, _) in model.files.iter().enumerate() {
            let managed = model.managed(copy);
            let follows = match &managed {
                Some(managed) => !managed.edited,
                None => model.copy_of(copy, index),
            };
            if copy == index || model.files[copy].kind != FileKind::File {
                continue;
            }
            if follows {
                builder.write(
                    copy,
                    bytes.clone(),
                    Some(format!("Managed copy of {}", model.files[index].display)),
                    Vec::new(),
                );
                copies.insert(model.files[copy].key.clone(), safe_fs::hash(&bytes));
            } else if managed.is_some() {
                builder.notes.push(format!(
                    "{} was edited by hand, so it keeps its own text. Restore or adopt it to make it a copy again.",
                    model.files[copy].display
                ));
            }
        }
        if let Some(record) = &model.record {
            for (key, hash) in &record.copies {
                copies.entry(key.clone()).or_insert_with(|| hash.clone());
            }
        }
        manifest = Some(Some(manifest::Entry {
            source: model.files[index].key.clone(),
            source_hash: safe_fs::hash(&bytes),
            copies,
        }));
    } else if let Some(record) = &model.record
        && record.copies.contains_key(&model.files[index].key)
    {
        let mut entry = record.clone();
        entry.copies.remove(&model.files[index].key);
        builder.notes.push(format!(
            "{} stops being a managed copy and keeps this text as its own.",
            model.files[index].display
        ));
        manifest = Some(Some(entry));
    }
    let title = format!("Save {}", model.files[index].display);
    let summary = describe_readers(model, index);
    builder.finish(title, summary, manifest)
}

fn describe_readers(model: &Model, index: usize) -> String {
    let readers = AGENTS
        .iter()
        .filter(|agent| {
            model.loaded(agent).is_some_and(|loaded| {
                loaded.files.contains(&index)
                    || loaded
                        .files
                        .iter()
                        .any(|file| agent.imports && model.import_of(*file) == Some(index))
            })
        })
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    if readers.is_empty() {
        format!("No agent reads {} here yet.", model.files[index].display)
    } else {
        format!(
            "{} read {}.",
            names(model, &readers),
            model.files[index].display
        )
    }
}

/// The text a switch to Universal settles on.
fn resolve_content(
    model: &Model,
    source: usize,
    content: &Content,
) -> Result<(String, Option<String>)> {
    let candidates = model.conflict(source);
    match content {
        Content::File(key) => {
            let index = known(model, key)?;
            let text = model.content(index).ok_or_else(|| {
                Error::Unsupported(format!(
                    "{} has no text to use.",
                    model.files[index].display
                ))
            })?;
            Ok((
                text.to_owned(),
                Some(format!("Keeps the text of {}.", model.files[index].display)),
            ))
        }
        Content::Combine if candidates.len() > 1 => {
            let parts = candidates
                .iter()
                .filter_map(|index| model.content(*index).map(normalize))
                .collect::<Vec<_>>();
            Ok((
                parts.join("\n\n") + "\n",
                Some(format!(
                    "Joins the text of {}, in that order.",
                    candidates
                        .iter()
                        .map(|index| model.files[*index].display.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ))
        }
        _ => {
            if model.has_text(source) {
                return Ok((model.content(source).unwrap_or_default().to_owned(), None));
            }
            // An empty or missing source takes the one other text there is, said in the review.
            let others = (0..model.files.len())
                .filter(|index| {
                    *index != source
                        && model.files[*index].kind == FileKind::File
                        && model.import_of(*index).is_none()
                        && model.has_text(*index)
                        && model.files[*index].path != "CLAUDE.local.md"
                })
                .collect::<Vec<_>>();
            let mut texts: Vec<String> = Vec::new();
            for index in &others {
                let text = normalize(model.content(*index).unwrap_or_default());
                if !texts.contains(&text) {
                    texts.push(text);
                }
            }
            match (texts.len(), others.first()) {
                (0, _) => Err(Error::Unsupported(format!(
                    "There is no text yet. Write the shared instructions in {} and save them first.",
                    model.files[source].display
                ))),
                (1, Some(first)) => Ok((
                    model.content(*first).unwrap_or_default().to_owned(),
                    Some(format!(
                        "{} takes the text of {}.",
                        model.files[source].display, model.files[*first].display
                    )),
                )),
                _ => Err(Error::Unsupported(
                    "These files have different text. Choose which to keep, or combine them."
                        .into(),
                )),
            }
        }
    }
}

fn universal(
    model: &Model,
    source_key: &str,
    content: &Content,
    remove: &[String],
    agents: Option<&[String]>,
) -> Result<Prepared> {
    let source = known(model, source_key)?;
    if !model.source_choices().contains(&source) {
        return Err(Error::Unsupported(
            "Every agent here reads AGENTS.md, so it holds the shared text.".into(),
        ));
    }
    let mut builder = Builder::new(model);
    let (value, content_note) = resolve_content(model, source, content)?;
    if let Some(note) = content_note {
        builder.notes.push(note);
    }
    let encoding = model.files[source].encoding.unwrap_or_default();
    let source_bytes = if model.has_text(source)
        && model.content(source).map(normalize) == Some(normalize(&value))
    {
        model.files[source].bytes.clone().unwrap_or_default()
    } else {
        let bytes = text::encode(&value, encoding);
        let notes = limit_notes(model, source, &value, bytes.len());
        builder.write(
            source,
            bytes.clone(),
            Some("Holds the shared text".into()),
            notes,
        );
        bytes
    };
    let removals = remove
        .iter()
        .map(|key| known(model, key))
        .collect::<Result<Vec<_>>>()?;
    if removals.contains(&source) {
        return Err(Error::Unsupported(
            "The shared file itself can't be removed.".into(),
        ));
    }
    let targeted = |agent: &AgentRule| match agents {
        Some(ids) => ids.iter().any(|id| id == agent.id),
        None => model.installed(agent),
    };
    let source_display = model.files[source].display.clone();
    let mut outcome = Outcome::default();
    let mut copies = BTreeMap::new();
    // Claude Code first: its import file changes what the agents after it see.
    let mut order = AGENTS.iter().collect::<Vec<_>>();
    order.sort_by_key(|agent| agent.id != "claude");
    for agent in order {
        let Some(reads) = model.reads(agent) else {
            if targeted(agent) {
                outcome.skipped.push(agent.id);
            }
            continue;
        };
        if !targeted(agent) {
            continue;
        }
        if reads.pick == Pick::ClaudeCode {
            claude_project(&mut builder, model, source, &removals, agent, &mut outcome)?;
            continue;
        }
        let position = |index: usize| {
            let file = &model.files[index];
            reads
                .files
                .iter()
                .position(|(root, path)| model.roots[file.root].id == *root && file.path == *path)
        };
        let own = model
            .by_path(reads.own.0, reads.own.1)
            .ok_or(Error::UnsafePath)?;
        let candidates = reads
            .files
            .iter()
            .filter_map(|(root, path)| model.by_path(*root, path))
            .collect::<Vec<_>>();
        if reads.pick == Pick::All {
            if position(source).is_some() {
                outcome.direct.push(agent.id);
            } else {
                copy_into(
                    &mut builder,
                    model,
                    own,
                    &source_bytes,
                    &source_display,
                    &mut copies,
                );
                outcome.copied.push(agent.id);
            }
            for extra in candidates {
                if extra != source
                    && extra != own
                    && model.has_text(extra)
                    && model.import_of(extra).is_none()
                    && model.files[extra].link_to != Some(source)
                    && !model.copy_of(extra, source)
                {
                    offer(&mut builder, model, extra, &removals, || {
                        format!(
                            "{} also reads {}. Remove it to keep one source; it is backed up.",
                            agent.name, model.files[extra].display
                        )
                    });
                }
            }
            continue;
        }
        // First match wins: find what this agent will read once the planned writes land.
        let pick_first = |builder: &Builder, removed: &[usize]| {
            candidates.iter().copied().find(|index| {
                exists_after(builder, model, *index, removed)
                    && !(reads.pick == Pick::FirstNonEmpty && blank_after(builder, model, *index))
            })
        };
        let first = pick_first(&builder, &removals);
        // A file the person chose to remove still counts as the override it was.
        if let Some(shadow) = pick_first(&builder, &[])
            && Some(shadow) != first
            && removals.contains(&shadow)
        {
            offer(&mut builder, model, shadow, &removals, String::new);
        }
        let follows = |index: usize| {
            index == source
                || model.files[index].link_to == Some(source)
                || agent.imports && model.project() && model.import_of(index) == Some(source)
        };
        match first {
            Some(index) if follows(index) => outcome.direct.push(agent.id),
            Some(index) if position(own).unwrap_or(usize::MAX) <= position(index).unwrap_or(0) => {
                if agent.id == "claude" {
                    point(&mut builder, model, own, source, agent);
                    outcome.imported.push(agent.id);
                } else {
                    copy_into(
                        &mut builder,
                        model,
                        own,
                        &source_bytes,
                        &source_display,
                        &mut copies,
                    );
                    outcome.copied.push(agent.id);
                }
            }
            Some(index) => {
                // An override or higher-priority file sits in front of the shared one.
                let target = if position(source).is_some() {
                    source
                } else {
                    own
                };
                offer(&mut builder, model, index, &removals, || {
                    format!(
                        "{} keeps reading {} instead of {}. Remove it to change that; it is backed up.",
                        agent.name, model.files[index].display, model.files[target].display
                    )
                });
                if removals.contains(&index) {
                    if target == own && own != source {
                        copy_into(
                            &mut builder,
                            model,
                            own,
                            &source_bytes,
                            &source_display,
                            &mut copies,
                        );
                        outcome.copied.push(agent.id);
                    } else {
                        outcome.direct.push(agent.id);
                    }
                } else {
                    outcome.kept.push(agent.id);
                }
            }
            None => {
                if position(source).is_some() {
                    outcome.direct.push(agent.id);
                } else if agent.id == "claude" {
                    point(&mut builder, model, own, source, agent);
                    outcome.imported.push(agent.id);
                } else {
                    copy_into(
                        &mut builder,
                        model,
                        own,
                        &source_bytes,
                        &source_display,
                        &mut copies,
                    );
                    outcome.copied.push(agent.id);
                }
            }
        }
    }
    // Managed copies outside the chosen agents follow the source too, so none falls behind.
    if let Some(record) = &model.record {
        for key in record.copies.keys() {
            if let Some(copy) = model.file(key)
                && copy != source
                && model.files[copy].kind == FileKind::File
                && !copies.contains_key(key)
            {
                copy_into(
                    &mut builder,
                    model,
                    copy,
                    &source_bytes,
                    &source_display,
                    &mut copies,
                );
            }
        }
    }
    for removal in &removals {
        if !builder.optional.contains(&model.files[*removal].key) {
            return Err(Error::Unsupported(format!(
                "{} isn't in the way of the shared file, so it stays.",
                model.files[*removal].display
            )));
        }
        builder.remove(*removal, vec!["Backed up in Ghost's data folder.".into()]);
    }
    let mut parts = vec![format!("{source_display} holds the shared text.")];
    let verb = |ids: &[&str], one: &str, many: &str| {
        if ids.len() == 1 {
            one.to_owned()
        } else {
            many.to_owned()
        }
    };
    if !outcome.direct.is_empty() {
        parts.push(format!(
            "{} {} it directly.",
            names(model, &outcome.direct),
            verb(&outcome.direct, "reads", "read")
        ));
    }
    if !outcome.imported.is_empty() {
        parts.push(format!(
            "{} {} it through an import.",
            names(model, &outcome.imported),
            verb(&outcome.imported, "reads", "read")
        ));
    }
    if !outcome.copied.is_empty() {
        parts.push(format!(
            "{} {} a copy Ghost keeps in sync.",
            names(model, &outcome.copied),
            verb(&outcome.copied, "gets", "get")
        ));
    }
    if !outcome.kept.is_empty() {
        parts.push(format!(
            "{} {} its override.",
            names(model, &outcome.kept),
            verb(&outcome.kept, "keeps", "keep")
        ));
    }
    if !outcome.skipped.is_empty() {
        builder.notes.push(format!(
            "{} {} no file at this scope, so {} left out.",
            names(model, &outcome.skipped),
            verb(&outcome.skipped, "has", "have"),
            verb(&outcome.skipped, "it is", "they are")
        ));
    }
    let entry = manifest::Entry {
        source: model.files[source].key.clone(),
        source_hash: safe_fs::hash(&source_bytes),
        copies,
    };
    builder.finish(
        "Use one file for every agent".into(),
        parts.join(" "),
        Some(Some(entry)),
    )
}

#[derive(Default)]
struct Outcome {
    direct: Vec<&'static str>,
    imported: Vec<&'static str>,
    copied: Vec<&'static str>,
    kept: Vec<&'static str>,
    skipped: Vec<&'static str>,
}

fn exists_after(builder: &Builder, model: &Model, index: usize, removals: &[usize]) -> bool {
    match builder.written(index) {
        Some(Write::Bytes(_) | Write::Rename) => true,
        Some(Write::Remove) => false,
        None => model.files[index].exists() && !removals.contains(&index),
    }
}

fn blank_after(builder: &Builder, model: &Model, index: usize) -> bool {
    match builder.written(index) {
        Some(Write::Bytes(bytes)) => {
            text::decode(bytes).is_some_and(|(text, _)| text.trim().is_empty())
        }
        _ => !model.has_text(index),
    }
}

/// Lists a file the person may remove, once, with the reason it matters.
fn offer(
    builder: &mut Builder,
    model: &Model,
    index: usize,
    removals: &[usize],
    note: impl FnOnce() -> String,
) {
    let key = model.files[index].key.clone();
    if !builder.optional.contains(&key) {
        builder.optional.push(key);
        if !removals.contains(&index) {
            builder.notes.push(note());
        }
    }
}

fn copy_into(
    builder: &mut Builder,
    model: &Model,
    index: usize,
    bytes: &[u8],
    source: &str,
    copies: &mut BTreeMap<String, String>,
) {
    let mut notes = Vec::new();
    if model.has_text(index) && !model.copy_of_bytes(index, bytes) {
        notes.push("Its earlier text is backed up in Ghost's data folder.".into());
    }
    builder.write(
        index,
        bytes.to_vec(),
        Some(format!("Managed copy of {source}")),
        notes,
    );
    copies.insert(model.files[index].key.clone(), safe_fs::hash(bytes));
}

/// Claude Code in a project: it reads AGENTS.md itself only when no CLAUDE file is in this
/// folder or above it; otherwise CLAUDE.md imports the shared file.
fn claude_project(
    builder: &mut Builder,
    model: &Model,
    source: usize,
    removals: &[usize],
    agent: &AgentRule,
    outcome: &mut Outcome,
) -> Result<()> {
    let present = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"]
        .iter()
        .filter_map(|path| model.by_path(RootId::Project, path))
        .filter(|index| model.files[*index].exists())
        .collect::<Vec<_>>();
    let own = model
        .by_path(RootId::Project, "CLAUDE.md")
        .ok_or(Error::UnsafePath)?;
    for extra in &present {
        let path = model.files[*extra].path;
        if *extra != own
            && path != "CLAUDE.local.md"
            && model.has_text(*extra)
            && model.import_of(*extra) != Some(source)
        {
            offer(builder, model, *extra, removals, || {
                format!(
                    "Claude Code also reads {path}. Remove it to keep one source; it is backed up."
                )
            });
        }
    }
    let remaining = present
        .iter()
        .filter(|index| !removals.contains(index))
        .count();
    if remaining == 0 && model.parent_claude.is_none() {
        outcome.direct.push(agent.id);
        return Ok(());
    }
    point(builder, model, own, source, agent);
    outcome.imported.push(agent.id);
    if model.import_of(own) != Some(source) && model.files[own].link_to != Some(source) {
        builder.notes.push(
            "Cursor and Grok Build read CLAUDE.md as plain text, so they see only its import line next to AGENTS.md.".into(),
        );
    }
    Ok(())
}

/// Makes `own` import `source` (Claude Code), keeping a file that already does.
fn point(builder: &mut Builder, model: &Model, own: usize, source: usize, agent: &AgentRule) {
    if model.import_of(own) == Some(source) || model.files[own].link_to == Some(source) {
        return;
    }
    let path = if model.project() {
        imports::relative(model.files[own].path, model.files[source].path)
    } else {
        let absolute = model.roots[model.files[source].root]
            .path
            .join(model.files[source].path);
        super::display_path(&absolute, model.context.home.as_deref())
    };
    let encoding = model.files[own]
        .encoding
        .or(model.files[source].encoding)
        .unwrap_or_default();
    let mut notes = Vec::new();
    if model.has_text(own) {
        notes.push("Its earlier text is backed up in Ghost's data folder.".into());
    }
    builder.write(
        own,
        text::encode(&imports::line(&path), encoding),
        Some(format!(
            "{} imports {}",
            agent.name, model.files[source].display
        )),
        notes,
    );
}

fn per_agent(model: &Model) -> Result<Prepared> {
    let (_, source) = model.mode();
    let mut builder = Builder::new(model);
    for (index, file) in model.files.iter().enumerate() {
        if let Some(target) = model.import_of(index)
            && let Some(bytes) = model.files[target].bytes.clone()
        {
            builder.write(
                index,
                bytes,
                Some(format!(
                    "Own copy of the text in {}",
                    model.files[target].display
                )),
                vec!["Edit it separately from now on.".into()],
            );
        } else if matches!(file.kind, FileKind::Link | FileKind::HardLink)
            && let Some(target) = file.link_to
            && let Some(bytes) = model.files[target].bytes.clone()
        {
            builder.write(
                index,
                bytes,
                Some(format!(
                    "Own copy of the text in {}",
                    model.files[target].display
                )),
                Vec::new(),
            );
        }
    }
    let mut keeps = Vec::new();
    if let Some(record) = &model.record {
        for key in record.copies.keys() {
            if let Some(copy) = model.file(key)
                && model.files[copy].exists()
            {
                keeps.push(copy);
            }
        }
    }
    // Claude Code reading AGENTS.md itself gets a CLAUDE.md of its own to edit.
    if model.project()
        && let Some(claude) = super::agents::rule("claude")
        && let Some(loaded) = model.loaded(claude)
        && let Some(agents) = model.by_path(RootId::Project, "AGENTS.md")
        && loaded.files == [agents]
        && let Some(own) = model.by_path(RootId::Project, "CLAUDE.md")
        && let Some(bytes) = model.files[agents].bytes.clone()
    {
        builder.write(
            own,
            bytes,
            Some("Claude Code's own file, starting from the shared text".into()),
            vec!["Cursor and Grok Build also read CLAUDE.md, so they see it too.".into()],
        );
    }
    let mut prepared_keep = Vec::new();
    for copy in keeps {
        if builder.written(copy).is_none() {
            prepared_keep.push(copy);
        }
    }
    let manifest = model.record.as_ref().map(|_| None);
    if builder.writes.is_empty() && prepared_keep.is_empty() {
        return Err(Error::Unsupported(
            "Every agent already reads its own file.".into(),
        ));
    }
    let summary = match source {
        Some(source) => format!(
            "Each agent keeps the text it reads now, as a file of its own. {} stays as it is.",
            model.files[source].display
        ),
        None => "Each agent keeps the text it reads now, as a file of its own.".into(),
    };
    let mut prepared = builder.finish("Use a file per agent".into(), summary, manifest)?;
    for copy in prepared_keep {
        prepared.review.files.push(ReviewFile {
            key: model.files[copy].key.clone(),
            path: model.files[copy].display.clone(),
            action: Action::Keep,
            before: model.files[copy].text.clone(),
            after: model.files[copy].text.clone(),
            how: Some("Stops being a managed copy; its text stays".into()),
            notes: Vec::new(),
            backup: false,
        });
    }
    Ok(prepared)
}

fn managed_copy(model: &Model, key: &str) -> Result<(usize, usize)> {
    let copy = known(model, key)?;
    let record = model.record.as_ref().ok_or_else(|| {
        Error::Unsupported(format!(
            "{} isn't a managed copy.",
            model.files[copy].display
        ))
    })?;
    if !record.copies.contains_key(&model.files[copy].key) {
        return Err(Error::Unsupported(format!(
            "{} isn't a managed copy.",
            model.files[copy].display
        )));
    }
    let source = model.file(&record.source).ok_or(Error::Stale)?;
    Ok((copy, source))
}

fn restore(model: &Model, key: &str) -> Result<Prepared> {
    let (copy, source) = managed_copy(model, key)?;
    let bytes = model.files[source].bytes.clone().ok_or(Error::Stale)?;
    let mut builder = Builder::new(model);
    builder.write(
        copy,
        bytes.clone(),
        Some(format!("Managed copy of {}", model.files[source].display)),
        vec!["The edited text is backed up in Ghost's data folder.".into()],
    );
    let mut entry = model.record.clone().ok_or(Error::Stale)?;
    entry
        .copies
        .insert(model.files[copy].key.clone(), safe_fs::hash(&bytes));
    entry.source_hash = safe_fs::hash(&bytes);
    builder.finish(
        format!("Restore {}", model.files[copy].display),
        format!(
            "{} gets the text of {} again.",
            model.files[copy].display, model.files[source].display
        ),
        Some(Some(entry)),
    )
}

fn adopt(model: &Model, key: &str) -> Result<Prepared> {
    let (copy, source) = managed_copy(model, key)?;
    let bytes = model.files[copy].bytes.clone().ok_or(Error::Stale)?;
    let mut builder = Builder::new(model);
    builder.write(
        source,
        bytes.clone(),
        Some("Holds the shared text".into()),
        Vec::new(),
    );
    let mut entry = model.record.clone().ok_or(Error::Stale)?;
    for other in entry.copies.keys().cloned().collect::<Vec<_>>() {
        if let Some(index) = model.file(&other)
            && index != copy
            && model.files[index].kind == FileKind::File
        {
            builder.write(
                index,
                bytes.clone(),
                Some(format!("Managed copy of {}", model.files[source].display)),
                Vec::new(),
            );
        }
        entry.copies.insert(other, safe_fs::hash(&bytes));
    }
    entry.source_hash = safe_fs::hash(&bytes);
    builder.finish(
        format!("Use the edit in {}", model.files[copy].display),
        format!(
            "{} and every managed copy take the text edited in {}.",
            model.files[source].display, model.files[copy].display
        ),
        Some(Some(entry)),
    )
}

fn rename(model: &Model, key: &str) -> Result<Prepared> {
    let index = known(model, key)?;
    let file = &model.files[index];
    let Some(disk) = &file.disk else {
        return Err(Error::Unsupported(format!(
            "{} already has the agents' spelling.",
            file.display
        )));
    };
    if file.kind != FileKind::File || file.read_only {
        return Err(Error::Unsupported(
            model
                .blocked(index)
                .unwrap_or_else(|| format!("{disk} can't be renamed here.")),
        ));
    }
    let mut builder = Builder::new(model);
    builder.writes.push((
        index,
        Write::Rename,
        Some(format!("Renamed from {disk}")),
        vec!["Git sees a rename. Commit it so other computers get the new spelling.".into()],
    ));
    builder.finish(
        format!("Rename {disk}"),
        format!("{disk} becomes {}. Its text stays the same.", file.path),
        None,
    )
}
