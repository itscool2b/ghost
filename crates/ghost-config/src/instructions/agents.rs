//! What each catalog agent reads, as data. Every rule cites the vendor's own documentation or
//! source (docs/instructions.md lists them); anything a vendor does not document is left out
//! rather than guessed.

/// A folder that holds instruction files: the project folder, or one agent's user folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum RootId {
    Project,
    Claude,
    Codex,
    Pi,
    Omp,
    OpenCode,
    Gemini,
    Grok,
}

impl RootId {
    pub const USER: [Self; 7] = [
        Self::Claude,
        Self::Codex,
        Self::Pi,
        Self::Omp,
        Self::OpenCode,
        Self::Gemini,
        Self::Grok,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Grok => "grok",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Project]
            .into_iter()
            .chain(Self::USER)
            .find(|root| root.as_str() == value)
    }
}

/// How an agent treats several candidate files in one folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The first file that exists wins; the rest are ignored.
    First,
    /// The first non-empty file wins (Codex skips empty files).
    FirstNonEmpty,
    /// Every file that exists is loaded.
    All,
    /// Claude Code: its CLAUDE files when any exists here or in a parent folder, else AGENTS.md.
    ClaudeCode,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Reads {
    pub files: &'static [(RootId, &'static str)],
    pub pick: Pick,
    /// The file Ghost creates for this agent: where an import or a managed copy goes.
    pub own: (RootId, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Limit {
    /// Bytes across every loaded file (Codex `project_doc_max_bytes`).
    CombinedBytes(usize),
    /// Bytes per file, after imports (Antigravity truncates).
    FileBytes(usize),
    /// Characters per file at the smallest context window (Hermes keeps head and tail).
    FileChars(usize),
    /// A hard skip: the whole file is ignored (Claude Code, 4 MiB).
    SkipOver(usize),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentRule {
    pub id: &'static str,
    pub name: &'static str,
    /// Catalog ids that run this agent's own CLI (adapters and text fallbacks included).
    pub catalog: &'static [&'static str],
    pub project: Reads,
    pub user: Option<Reads>,
    /// The agent expands `@path` imports in its own files.
    pub imports: bool,
    pub limits: &'static [Limit],
    /// Soft guidance from the vendor, in lines.
    pub recommended_lines: Option<usize>,
    /// Said when this agent has no user-level instructions file.
    pub no_user_file: &'static str,
    /// Files this agent loads only when they are not ignored by Git (Grok Build).
    pub skips_ignored: bool,
}

use Pick::*;
use RootId::*;

pub(crate) const CODEX_LIMIT: usize = 32 * 1024;

pub(crate) const AGENTS: &[AgentRule] = &[
    AgentRule {
        id: "claude",
        name: "Claude Code",
        catalog: &["claude", "claude-acp"],
        project: Reads {
            files: &[
                (Project, "CLAUDE.md"),
                (Project, ".claude/CLAUDE.md"),
                (Project, "CLAUDE.local.md"),
                (Project, "AGENTS.md"),
            ],
            pick: ClaudeCode,
            own: (Project, "CLAUDE.md"),
        },
        user: Some(Reads {
            files: &[(Claude, "CLAUDE.md")],
            pick: First,
            own: (Claude, "CLAUDE.md"),
        }),
        imports: true,
        limits: &[Limit::SkipOver(4 * 1024 * 1024)],
        recommended_lines: Some(200),
        no_user_file: "",
        skips_ignored: false,
    },
    AgentRule {
        id: "codex",
        name: "Codex",
        catalog: &["codex", "codex-acp"],
        project: Reads {
            files: &[(Project, "AGENTS.override.md"), (Project, "AGENTS.md")],
            pick: FirstNonEmpty,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[(Codex, "AGENTS.override.md"), (Codex, "AGENTS.md")],
            pick: FirstNonEmpty,
            own: (Codex, "AGENTS.md"),
        }),
        imports: false,
        limits: &[Limit::CombinedBytes(CODEX_LIMIT)],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: false,
    },
    AgentRule {
        id: "pi",
        name: "Pi",
        catalog: &["pi"],
        project: Reads {
            files: &[
                (Project, "AGENTS.override.md"),
                (Project, "AGENTS.md"),
                (Project, "CLAUDE.md"),
            ],
            pick: First,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[
                (Pi, "AGENTS.override.md"),
                (Pi, "AGENTS.md"),
                (Pi, "CLAUDE.md"),
            ],
            pick: First,
            own: (Pi, "AGENTS.md"),
        }),
        imports: false,
        limits: &[],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: false,
    },
    AgentRule {
        id: "omp",
        name: "oh-my-pi",
        catalog: &["omp"],
        project: Reads {
            files: &[
                (Project, ".omp/AGENTS.md"),
                (Project, ".claude/CLAUDE.md"),
                (Project, ".agents/AGENTS.md"),
                (Project, ".agent/AGENTS.md"),
                (Project, ".gemini/GEMINI.md"),
                (Project, ".github/copilot-instructions.md"),
                (Project, "AGENTS.md"),
                (Project, "CLAUDE.md"),
            ],
            pick: First,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[
                (Omp, "AGENTS.md"),
                (Claude, "CLAUDE.md"),
                (Codex, "AGENTS.md"),
                (Gemini, "GEMINI.md"),
                (OpenCode, "AGENTS.md"),
            ],
            pick: First,
            own: (Omp, "AGENTS.md"),
        }),
        imports: true,
        limits: &[],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: false,
    },
    AgentRule {
        id: "opencode",
        name: "OpenCode",
        catalog: &["opencode", "opencode-v2"],
        project: Reads {
            files: &[(Project, "AGENTS.md"), (Project, "CLAUDE.md")],
            pick: First,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[(OpenCode, "AGENTS.md"), (Claude, "CLAUDE.md")],
            pick: First,
            own: (OpenCode, "AGENTS.md"),
        }),
        imports: false,
        limits: &[],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: false,
    },
    AgentRule {
        id: "hermes",
        name: "Hermes",
        catalog: &["hermes"],
        project: Reads {
            files: &[
                (Project, ".hermes.md"),
                (Project, "HERMES.md"),
                (Project, "AGENTS.override.md"),
                (Project, "AGENTS.md"),
                (Project, "CLAUDE.md"),
                (Project, ".cursorrules"),
            ],
            pick: First,
            own: (Project, "AGENTS.md"),
        },
        user: None,
        imports: false,
        limits: &[Limit::FileChars(20_000)],
        recommended_lines: None,
        no_user_file: "Hermes has no user-level instructions file. Its SOUL.md sets the agent's identity, not project rules.",
        skips_ignored: false,
    },
    AgentRule {
        id: "cursor",
        name: "Cursor",
        catalog: &["cursor"],
        project: Reads {
            files: &[(Project, "AGENTS.md"), (Project, "CLAUDE.md")],
            pick: All,
            own: (Project, "AGENTS.md"),
        },
        user: None,
        imports: false,
        limits: &[],
        recommended_lines: Some(500),
        no_user_file: "Cursor keeps user rules in its settings and in ~/.cursor/rules, not in one file.",
        skips_ignored: false,
    },
    AgentRule {
        id: "grok",
        name: "Grok Build",
        catalog: &["grok", "grok-stream"],
        project: Reads {
            files: &[
                (Project, "AGENTS.md"),
                (Project, "AGENT.md"),
                (Project, "CLAUDE.md"),
                (Project, "CLAUDE.local.md"),
                (Project, ".claude/CLAUDE.md"),
                (Project, ".claude/CLAUDE.local.md"),
            ],
            pick: All,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[(Grok, "rules/AGENTS.md")],
            pick: All,
            own: (Grok, "rules/AGENTS.md"),
        }),
        imports: false,
        limits: &[],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: true,
    },
    AgentRule {
        id: "antigravity",
        name: "Antigravity",
        catalog: &["antigravity", "agy-stream"],
        project: Reads {
            files: &[
                (Project, "AGENTS.md"),
                (Project, "GEMINI.md"),
                (Project, ".agents/AGENTS.md"),
                (Project, ".agents/GEMINI.md"),
            ],
            pick: All,
            own: (Project, "AGENTS.md"),
        },
        user: Some(Reads {
            files: &[(Gemini, "AGENTS.md"), (Gemini, "GEMINI.md")],
            pick: All,
            own: (Gemini, "AGENTS.md"),
        }),
        imports: false,
        limits: &[Limit::FileBytes(24_000)],
        recommended_lines: None,
        no_user_file: "",
        skips_ignored: false,
    },
];

/// Every file a scan looks at for a scope, without duplicates, in a stable order.
pub(crate) fn known(project: bool) -> Vec<(RootId, &'static str)> {
    let mut files = Vec::new();
    for agent in AGENTS {
        let reads = if project {
            Some(agent.project)
        } else {
            agent.user
        };
        if let Some(reads) = reads {
            for file in reads.files.iter().chain([&reads.own]) {
                if !files.contains(file) {
                    files.push(*file);
                }
            }
        }
    }
    files.sort_by_key(|(root, path)| (*root, rank(path), *path));
    files
}

/// The files people recognise first.
fn rank(path: &str) -> u8 {
    match path {
        "AGENTS.md" => 0,
        "CLAUDE.md" => 1,
        "AGENTS.override.md" => 2,
        "GEMINI.md" => 3,
        "CLAUDE.local.md" => 4,
        _ if !path.contains('/') => 5,
        _ => 6,
    }
}

pub(crate) fn rule(id: &str) -> Option<&'static AgentRule> {
    AGENTS.iter().find(|agent| agent.id == id)
}

/// The instruction agent a catalog id runs.
pub(crate) fn for_catalog(id: &str) -> Option<&'static AgentRule> {
    AGENTS.iter().find(|agent| agent.catalog.contains(&id))
}

/// Files that only exist to change one agent's precedence: removing one is never a default.
pub(crate) fn is_override(path: &str) -> bool {
    path.ends_with("AGENTS.override.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_fs;

    #[test]
    fn every_known_path_is_safe_and_every_catalog_agent_maps_once() {
        for project in [true, false] {
            for (_, path) in known(project) {
                safe_fs::validate(path).unwrap();
            }
        }
        let mut seen = std::collections::HashSet::new();
        for agent in AGENTS {
            for id in agent.catalog {
                assert!(seen.insert(*id), "{id} mapped twice");
            }
            assert!(agent.project.files.contains(&agent.project.own) || agent.id == "claude");
        }
        assert_eq!(for_catalog("codex-acp").unwrap().id, "codex");
        assert!(for_catalog("mock").is_none());
        assert_eq!(known(true)[0], (Project, "AGENTS.md"));
    }
}
