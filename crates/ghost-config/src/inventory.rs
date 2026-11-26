use std::{collections::HashSet, path::PathBuf};

use cap_std::fs::Dir;
use serde_json::Value;

use crate::{
    Error, Inventory, Item, ItemPreview, ItemRef, Kind, Provider, Result, Scope, Source,
    SourceStatus, mcp, safe_fs,
};

pub struct Library {
    pub(crate) sources: Vec<Source>,
}

pub(crate) struct Entry {
    pub item: Item,
    pub text: String,
    pub files: Vec<safe_fs::FileData>,
}

impl Library {
    pub fn new(sources: Vec<Source>) -> Result<Self> {
        let mut ids = HashSet::new();
        if sources.len() > 32
            || sources.iter().any(|source| {
                source.id.is_empty()
                    || source.id.len() > 128
                    || !source.root.is_absolute()
                    || !ids.insert(source.id.clone())
                    || source.claude_state_only
                        && (source.provider != Provider::Claude || source.scope != Scope::User)
            })
        {
            return Err(Error::UnsafePath);
        }
        Ok(Self { sources })
    }

    /// Explicit user action only. Does not inspect credential stores, run agents, or refresh trust.
    pub fn inspect(&self) -> Inventory {
        let mut result = Inventory::default();
        for source in &self.sources {
            let directory = open(source);
            result.sources.push(SourceStatus {
                id: source.id.clone(),
                provider: source.provider,
                scope: source.scope,
                available: directory.is_ok(),
                message: directory.as_ref().err().map(ToString::to_string),
            });
            let Ok(directory) = directory else { continue };
            match scan(source, &directory) {
                Ok(entries) => result
                    .items
                    .extend(entries.into_iter().map(|entry| entry.item)),
                Err(error) => result.notices.push(format!("{}: {error}", source.id)),
            }
        }
        result
    }

    pub fn preview(&self, reference: &ItemRef, revision: &str) -> Result<ItemPreview> {
        let entry = self.entry(reference)?;
        if entry.item.revision != revision {
            return Err(Error::Stale);
        }
        let mut notices = Vec::new();
        if entry.item.item.kind == Kind::Skill {
            notices.push("Skills and their supporting files are untrusted content. The Library does not execute them.".into());
            let peers = self
                .inspect()
                .items
                .into_iter()
                .filter(|item| {
                    item.item.kind == Kind::Skill
                        && item.item != entry.item.item
                        && item.item.key.rsplit('/').next()
                            == entry.item.item.key.rsplit('/').next()
                        && (item.provider == entry.item.provider
                            || item.provider == Provider::Shared
                            || entry.item.provider == Provider::Shared)
                })
                .collect::<Vec<_>>();
            if !peers.is_empty() {
                notices.push(format!("Other copies with this directory name: {}. Each copy remains separate; copying does not change the agent's discovery or trust rules.", peers.iter().map(|item| format!("{}: {}", item.item.source_id, item.relative_path)).collect::<Vec<_>>().join(", ")));
                notices.push(match entry.item.provider {
                    Provider::Claude => "Claude's personal skill takes precedence over a same-named project skill. Nested project skills have directory-qualified names; managed policy can override both.",
                    Provider::Codex => "Codex can show multiple same-named skills. This Library does not merge them or select an active winner.",
                    Provider::Pi => "Pi keeps the first discovered same-named skill and reports a warning. Its runtime discovery order determines which one loads.",
                    Provider::Shared => "Shared skills can be discovered by several agents. Each agent applies its own duplicate and project-trust rules.",
                }.into());
            }
        }
        if entry.item.item.kind == Kind::Plugin || entry.item.item.kind == Kind::Package {
            notices.push("This is an installation declaration, not proof that its package is installed or active.".into());
        }
        Ok(ItemPreview {
            item: entry.item,
            text: entry.text,
            files: entry.files.into_iter().map(|file| file.path).collect(),
            notices,
        })
    }

    pub(crate) fn entry(&self, reference: &ItemRef) -> Result<Entry> {
        let source = self
            .sources
            .iter()
            .find(|source| source.id == reference.source_id)
            .ok_or(Error::Unavailable)?;
        scan(source, &open(source)?)?
            .into_iter()
            .find(|entry| entry.item.item == *reference)
            .ok_or(Error::Stale)
    }
}

pub(crate) fn open(source: &Source) -> Result<Dir> {
    Dir::open_ambient_dir(&source.root, cap_std::ambient_authority())
        .map_err(|_| Error::Unavailable)
}

fn scan(source: &Source, root: &Dir) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    if source.claude_state_only {
        json_items(
            source,
            root,
            ".claude.json",
            "mcpServers",
            Kind::McpServer,
            &mut entries,
        )?;
        return Ok(entries);
    }
    let prefix = if source.scope == Scope::Project {
        match source.provider {
            Provider::Claude => ".claude/",
            Provider::Codex => ".codex/",
            Provider::Pi => ".pi/",
            Provider::Shared => "",
        }
    } else {
        ""
    };
    if source.provider == Provider::Shared {
        skills(source, root, "", &mut entries)?;
        return Ok(entries);
    }
    let instructions: &[&str] = match source.provider {
        Provider::Claude => &["CLAUDE.md"],
        Provider::Codex => &["AGENTS.md", "AGENTS.override.md"],
        Provider::Pi => &["AGENTS.md", "AGENTS.override.md", "CLAUDE.md"],
        Provider::Shared => &[],
    };
    for path in instructions {
        if let Some(bytes) = safe_fs::read(root, path)? {
            bounded_push(
                &mut entries,
                text_entry(source, Kind::Instructions, path, path, bytes)?,
            )?;
        }
    }
    if source.provider == Provider::Pi {
        for name in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
            let path = format!("{prefix}{name}");
            if let Some(bytes) = safe_fs::read(root, &path)? {
                let mut entry = text_entry(source, Kind::Instructions, name, &path, bytes)?;
                entry.item.copy_blocked = Some("Pi system prompts have provider-specific semantics and need a manual conversion.".into());
                bounded_push(&mut entries, entry)?;
            }
        }
    }
    skills(source, root, &format!("{prefix}skills"), &mut entries)?;
    match source.provider {
        Provider::Claude => {
            if source.scope == Scope::Project {
                json_items(
                    source,
                    root,
                    ".mcp.json",
                    "mcpServers",
                    Kind::McpServer,
                    &mut entries,
                )?;
            }
            json_items(
                source,
                root,
                &format!("{prefix}settings.json"),
                "enabledPlugins",
                Kind::Plugin,
                &mut entries,
            )?;
        }
        Provider::Codex => {
            let path = format!("{prefix}config.toml");
            if let Some(bytes) = safe_fs::read(root, &path)? {
                let doc = text(&bytes)?
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|_| Error::Format)?;
                for (section, kind) in [("mcp_servers", Kind::McpServer), ("plugins", Kind::Plugin)]
                {
                    if let Some(item) = doc.get(section) {
                        let table = item.as_table_like().ok_or(Error::Format)?;
                        for (key, item) in table.iter() {
                            push_value(
                                source,
                                &path,
                                key,
                                kind,
                                mcp::toml_json(item)?,
                                &mut entries,
                            )?;
                        }
                    }
                }
            }
        }
        Provider::Pi => {
            json_items(
                source,
                root,
                &format!("{prefix}mcp.json"),
                "mcpServers",
                Kind::McpServer,
                &mut entries,
            )?;
            let path = format!("{prefix}settings.json");
            if let Some(bytes) = safe_fs::read(root, &path)? {
                let doc: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
                if let Some(packages) = doc.get("packages") {
                    for (index, package) in
                        packages.as_array().ok_or(Error::Format)?.iter().enumerate()
                    {
                        // Package options can embed paths or source credentials. Show only a safe
                        // identifier, and leave installation to an explicit vendor workflow.
                        let name = package
                            .as_str()
                            .or_else(|| package.get("source").and_then(Value::as_str))
                            .ok_or(Error::Format)?;
                        let label = if package_source_safe(name) {
                            name.to_owned()
                        } else {
                            format!("Package {} (private source)", index + 1)
                        };
                        push_value(
                            source,
                            &path,
                            &label,
                            Kind::Package,
                            Value::Null,
                            &mut entries,
                        )?;
                    }
                }
            }
        }
        Provider::Shared => {}
    }
    Ok(entries)
}

fn package_source_safe(name: &str) -> bool {
    name.len() <= 256
        && name.starts_with("npm:")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/._-:^~".contains(c))
}

fn skills(source: &Source, root: &Dir, base: &str, entries: &mut Vec<Entry>) -> Result<()> {
    if !base.is_empty()
        && matches!(root.symlink_metadata(base), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    let recursive = matches!(source.provider, Provider::Pi | Provider::Shared);
    let mut pending = vec![(base.to_owned(), 0)];
    let mut visited = 0;
    while let Some((parent, depth)) = pending.pop() {
        for name in safe_fs::children(root, &parent)? {
            visited += 1;
            if visited > safe_fs::ITEM_COUNT {
                return Err(Error::Limit);
            }
            let folder = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            // Files, links, and folders without SKILL.md are not portable skills.
            if safe_fs::directory(root, &folder).is_err() {
                continue;
            }
            let path = format!("{folder}/SKILL.md");
            let Some(bytes) = safe_fs::read(root, &path)? else {
                if recursive {
                    if depth >= 7 {
                        return Err(Error::Limit);
                    }
                    pending.push((folder, depth + 1));
                }
                continue;
            };
            let key = if base.is_empty() {
                folder.as_str()
            } else {
                folder
                    .strip_prefix(base)
                    .and_then(|path| path.strip_prefix('/'))
                    .ok_or(Error::UnsafePath)?
            };
            let mut entry = text_entry(source, Kind::Skill, key, &folder, bytes)?;
            match safe_fs::tree(root, &folder) {
                Ok(files) => {
                    entry.item.revision = safe_fs::tree_hash(&files);
                    entry.item.bytes = files.iter().map(|file| file.bytes.len() as u32).sum();
                    entry.files = files;
                }
                Err(error) => {
                    entry.item.copy_blocked =
                        Some(format!("The complete skill cannot be copied: {error}"))
                }
            }
            bounded_push(entries, entry)?;
        }
    }
    Ok(())
}

fn text_entry(source: &Source, kind: Kind, key: &str, path: &str, bytes: Vec<u8>) -> Result<Entry> {
    let text = text(&bytes)?.to_owned();
    Ok(Entry {
        item: Item {
            item: ItemRef {
                source_id: source.id.clone(),
                kind,
                key: key.into(),
            },
            provider: source.provider,
            scope: source.scope,
            label: key.into(),
            relative_path: path.into(),
            revision: safe_fs::hash(&bytes),
            bytes: bytes.len() as u32,
            copy_blocked: None,
        },
        text,
        files: vec![safe_fs::FileData {
            path: PathBuf::from(path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(Error::UnsafePath)?
                .to_owned(),
            bytes,
            executable: false,
        }],
    })
}

pub(crate) fn text(bytes: &[u8]) -> Result<&str> {
    if bytes.contains(&0) {
        return Err(Error::Format);
    }
    std::str::from_utf8(bytes).map_err(|_| Error::Format)
}

fn json_items(
    source: &Source,
    root: &Dir,
    path: &str,
    key: &str,
    kind: Kind,
    entries: &mut Vec<Entry>,
) -> Result<()> {
    let Some(bytes) = safe_fs::read(root, path)? else {
        return Ok(());
    };
    let doc: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
    if let Some(items) = doc.get(key) {
        for (name, value) in items.as_object().ok_or(Error::Format)? {
            push_value(source, path, name, kind, value.clone(), entries)?;
        }
    }
    Ok(())
}

fn push_value(
    source: &Source,
    path: &str,
    key: &str,
    kind: Kind,
    value: Value,
    entries: &mut Vec<Entry>,
) -> Result<()> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(Error::Format);
    }
    let (text, blocked) = if kind == Kind::McpServer {
        mcp::preview(&value, source.provider)
    } else {
        // Plugin-specific configuration may contain userConfig secrets. Only enabled is safe to
        // expose; never serialize the whole plugin declaration to the webview or a backup.
        let enabled = value
            .as_bool()
            .or_else(|| value.get("enabled").and_then(Value::as_bool));
        (serde_json::to_string_pretty(&serde_json::json!({"name":key, "enabled":enabled})).map_err(|_| Error::Format)?,
            Some("Use the agent's explicit package installation flow. Plugin and package formats are not interchangeable.".into()))
    };
    let bytes = serde_json::to_vec(&value).map_err(|_| Error::Format)?;
    bounded_push(
        entries,
        Entry {
            item: Item {
                item: ItemRef {
                    source_id: source.id.clone(),
                    kind,
                    key: key.into(),
                },
                provider: source.provider,
                scope: source.scope,
                label: key.into(),
                relative_path: path.into(),
                revision: safe_fs::hash(&bytes),
                bytes: bytes.len() as u32,
                copy_blocked: blocked,
            },
            text,
            files: Vec::new(),
        },
    )
}

fn bounded_push(entries: &mut Vec<Entry>, entry: Entry) -> Result<()> {
    if entries.len() == safe_fs::ITEM_COUNT
        || entries
            .iter()
            .map(|entry| entry.item.bytes as usize)
            .sum::<usize>()
            + entry.item.bytes as usize
            > safe_fs::TREE_LIMIT
    {
        return Err(Error::Limit);
    }
    entries.push(entry);
    Ok(())
}
