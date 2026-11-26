//! Opt-in end-to-end checkpoint measurements using disposable workspaces.
use super::*;
use crate::profile::{ProfileKind, ProfileMeta};
use ghost_process::{EnvSource, ResolvedEnv, process_env};
use std::fmt::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

const DIAGNOSTIC_EVENT_LIMIT: usize = 512;
const DIAGNOSTIC_FIELD_LIMIT: usize = 24;
const DIAGNOSTIC_TEXT_LIMIT: usize = 64;

enum DiagnosticValue {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Boolean(bool),
    Text(String),
}

impl std::fmt::Display for DiagnosticValue {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsigned(value) => write!(output, "{value}"),
            Self::Signed(value) => write!(output, "{value}"),
            Self::Float(value) => write!(output, "{value}"),
            Self::Boolean(value) => write!(output, "{value}"),
            Self::Text(value) => write!(output, "{value:?}"),
        }
    }
}

struct DiagnosticEvent {
    target: &'static str,
    fields: Vec<(&'static str, DiagnosticValue)>,
}

impl DiagnosticEvent {
    fn accepts(&self, field: &Field) -> bool {
        self.fields.len() < DIAGNOSTIC_FIELD_LIMIT
            && matches!(
                field.name(),
                "stage"
                    | "phase"
                    | "operation"
                    | "elapsed_ms"
                    | "helper_count"
                    | "outcome"
                    | "storage"
                    | "file_count"
                    | "total_bytes"
                    | "copied_count"
                    | "reused_count"
                    | "cache_missing"
                    | "success"
                    | "fsync_method"
                    | "fsync_mode"
                    | "batch_fsync"
            )
    }

    fn push(&mut self, field: &Field, value: DiagnosticValue) {
        self.fields.push((field.name(), value));
    }
}

impl Visit for DiagnosticEvent {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if self.accepts(field) {
            self.push(field, DiagnosticValue::Unsigned(value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if self.accepts(field) {
            self.push(field, DiagnosticValue::Signed(value));
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if self.accepts(field) {
            self.push(field, DiagnosticValue::Float(value));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if self.accepts(field) {
            self.push(field, DiagnosticValue::Boolean(value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if self.accepts(field) {
            self.push(
                field,
                DiagnosticValue::Text(value.chars().take(DIAGNOSTIC_TEXT_LIMIT).collect()),
            );
        }
    }

    // Arbitrary Debug values and tracing's message field may contain paths or user content.
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

#[derive(Default)]
struct DiagnosticBuffer {
    events: Vec<DiagnosticEvent>,
    dropped: usize,
}

struct BufferedDiagnostics(Arc<Mutex<DiagnosticBuffer>>);

impl Subscriber for BufferedDiagnostics {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event()
            && (metadata.target() == "ghost_core::checkpoints"
                || metadata.target().starts_with("ghost_core::checkpoints::"))
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}

    fn event(&self, event: &Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut buffer = self.0.lock().unwrap();
        if buffer.events.len() == DIAGNOSTIC_EVENT_LIMIT {
            buffer.dropped = buffer.dropped.saturating_add(1);
            return;
        }
        let mut recorded = DiagnosticEvent {
            target: event.metadata().target(),
            fields: Vec::with_capacity(8),
        };
        event.record(&mut recorded);
        buffer.events.push(recorded);
    }
}

/// The comparison script runs one fixture at a time. A global subscriber also observes its
/// blocking scan workers. Collection adds bounded allocations and a short mutex acquisition;
/// formatting and stdout writes happen only after the measured capture has finished.
fn diagnostics() -> Option<Arc<Mutex<DiagnosticBuffer>>> {
    if std::env::var_os("GHOST_CHECKPOINT_BENCH_DIAGNOSTICS").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return None;
    }
    static BUFFER: OnceLock<Arc<Mutex<DiagnosticBuffer>>> = OnceLock::new();
    Some(Arc::clone(BUFFER.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(DiagnosticBuffer::default()));
        tracing::subscriber::set_global_default(BufferedDiagnostics(Arc::clone(&buffer)))
            .expect("checkpoint diagnostic fixture requires its own tracing subscriber");
        buffer
    })))
}

fn reset_diagnostics(diagnostics: &Option<Arc<Mutex<DiagnosticBuffer>>>) {
    if let Some(diagnostics) = diagnostics {
        let mut buffer = diagnostics.lock().unwrap();
        buffer.events.clear();
        buffer.dropped = 0;
    }
}

fn print_diagnostics(
    diagnostics: &Option<Arc<Mutex<DiagnosticBuffer>>>,
    fixture: &str,
    storage: &str,
    scenario: &str,
    sample: usize,
) {
    let Some(diagnostics) = diagnostics else {
        return;
    };
    let buffer = std::mem::take(&mut *diagnostics.lock().unwrap());
    for event in buffer.events {
        let mut line = format!(
            "CAPTURE_DIAGNOSTIC fixture={fixture} storage={storage} scenario={scenario} sample={sample} target={}",
            event.target
        );
        for (name, value) in event.fields {
            write!(line, " {name}={value}").unwrap();
        }
        println!("{line}");
    }
    if buffer.dropped > 0 {
        println!(
            "CAPTURE_DIAGNOSTIC fixture={fixture} storage={storage} scenario={scenario} sample={sample} dropped={}",
            buffer.dropped
        );
    }
}

#[tokio::test]
#[ignore = "manual read-only checkpoint measurement of an explicitly selected workspace"]
async fn checkpoint_workspace_performance() {
    let diagnostics = diagnostics();
    let work = PathBuf::from(
        std::env::var_os("GHOST_CHECKPOINT_BENCH_WORKSPACE").expect("select a workspace"),
    );
    let temp = tempfile::tempdir().unwrap();
    let profile = Profile {
        meta: ProfileMeta {
            id: crate::ids::new_id(),
            kind: ProfileKind::Local,
            display_name: "workspace benchmark".into(),
            created_at: 0,
        },
        dir: temp.path().join("profile"),
    };
    let env = EnvResolver::fixed(ResolvedEnv {
        vars: process_env(),
        source: EnvSource::Fallback {
            reason: "workspace benchmark".into(),
        },
        warnings: vec![],
        resolved_at_ms: 0,
    });
    let service = Checkpoints::new(&profile, env, LeftoverRegistry::disabled());
    let chat = crate::ids::new_id();
    let folder = crate::ids::new_id();
    for attempt in 0..4 {
        let scenario = if attempt == 0 { "cold" } else { "unchanged" };
        reset_diagnostics(&diagnostics);
        let started = Instant::now();
        let captured = service
            .capture(&chat, &folder, &work, &crate::ids::new_id())
            .await;
        let elapsed_ms = started.elapsed().as_millis();
        print_diagnostics(&diagnostics, "workspace", "local", scenario, attempt);
        let summary = captured.unwrap();
        println!(
            "CHECKPOINT_WORKSPACE storage=local files={} bytes={} scenario={scenario} elapsed_ms={elapsed_ms}",
            summary.file_count, summary.total_bytes,
        );
    }
}

#[tokio::test]
#[ignore = "manual checkpoint performance fixture"]
async fn checkpoint_performance() {
    let diagnostics = diagnostics();
    let warm_samples = std::env::var("GHOST_CHECKPOINT_BENCH_WARM")
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(1);
    assert!(warm_samples > 0);
    let counts = std::env::var("GHOST_CHECKPOINT_BENCH_COUNTS")
        .unwrap_or_else(|_| "1000,10000,20000".into());
    for count in counts
        .split(',')
        .map(|value| value.parse::<usize>().unwrap())
    {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        for index in 0..count {
            std::fs::write(
                work.join(format!("file-{index:05}.txt")),
                format!(
                    "fixture {index}\r\n{}",
                    "line of synthetic source\n".repeat(10)
                ),
            )
            .unwrap();
        }
        let profile = Profile {
            meta: ProfileMeta {
                id: crate::ids::new_id(),
                kind: ProfileKind::Local,
                display_name: "benchmark".into(),
                created_at: 0,
            },
            dir: temp.path().join("profile"),
        };
        let env = EnvResolver::fixed(ResolvedEnv {
            vars: process_env(),
            source: EnvSource::Fallback {
                reason: "checkpoint benchmark".into(),
            },
            warnings: vec![],
            resolved_at_ms: 0,
        });
        let service = Checkpoints::new(&profile, env, LeftoverRegistry::disabled());
        let chat = crate::ids::new_id();
        let folder = crate::ids::new_id();
        for attempt in 0..warm_samples + 2 {
            let scenario = match attempt {
                0 => "cold",
                value if value == warm_samples + 1 => "one-file",
                _ => "unchanged",
            };
            if attempt == warm_samples + 1 {
                std::fs::write(work.join("file-00000.txt"), "changed\r\n").unwrap();
            }
            reset_diagnostics(&diagnostics);
            let started = Instant::now();
            let captured = service
                .capture(&chat, &folder, &work, &crate::ids::new_id())
                .await;
            let elapsed_ms = started.elapsed().as_millis();
            let storage = "local";
            print_diagnostics(&diagnostics, "synthetic", storage, scenario, attempt);
            let summary = captured.unwrap();
            assert_eq!(summary.file_count as usize, count);
            println!(
                "CHECKPOINT_BENCH storage={storage} files={count} scenario={scenario} elapsed_ms={elapsed_ms}"
            );
        }
    }
}
