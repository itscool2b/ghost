//! A Godot project checked without running it: every script and scene is loaded headless by
//! Ghost's own check script (`resources/godot/check.gd`), and what does not parse or load is
//! reported with its file and line in Godot's own words. No window opens and nothing is changed.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::Error;

/// The longest a check may take. A project's first headless load imports its assets.
const PATIENCE: Duration = Duration::from_secs(180);
/// The most of Godot's output that is read.
const OUTPUT_LIMIT: u64 = 512 * 1024;
/// The most problems reported from one check.
const MAX_PROBLEMS: usize = 40;

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Scripts and scenes looked at.
    pub files: u32,
    /// How many of them did not load.
    pub failed: u32,
    /// Each problem as "res://file.gd:12: what Godot said", in the order Godot said them.
    pub problems: Vec<String>,
    /// The check ran to its end.
    pub finished: bool,
}

/// Reads what Godot printed during a check. Godot says an error on one line and where on the
/// next ("   at: GDScript::reload (res://player.gd:12)"); only places inside the project count,
/// since the rest are the engine's own source files.
pub fn read(output: &str) -> Report {
    let mut report = Report::default();
    let mut pending: Option<String> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("GHOST-CHECK-DONE ") {
            let mut numbers = rest
                .split_whitespace()
                .map(|n| n.parse::<u32>().unwrap_or(0));
            report.files = numbers.next().unwrap_or(0);
            report.failed = numbers.next().unwrap_or(0);
            report.finished = true;
            continue;
        }
        let said = [
            "SCRIPT ERROR: ",
            "ERROR: ",
            "USER SCRIPT ERROR: ",
            "USER ERROR: ",
        ]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix));
        if let Some(said) = said {
            pending = Some(said.chars().take(400).collect());
            continue;
        }
        if let Some(place) = trimmed.strip_prefix("at: ")
            && let Some(said) = pending.take()
            && let Some(start) = place.rfind("(res://")
            && let Some(inside) = place[start + 1..].strip_suffix(')')
            && report.problems.len() < MAX_PROBLEMS
        {
            let problem = format!("{inside}: {said}");
            if !report.problems.contains(&problem) {
                report.problems.push(problem);
            }
        }
    }
    report
}

/// Checks the project in `project` with `godot`, running Ghost's `check` script headless.
pub async fn run(godot: &Path, project: &Path, check: &Path) -> Result<Report, Error> {
    let mut command = crate::command(godot);
    command
        .arg("--headless")
        .arg("--path")
        .arg(project)
        .arg("-s")
        .arg(check)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| Error::Game(format!("Godot could not be started: {error}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read_all = async {
        let mut text = String::new();
        // Godot prints errors to its error stream and the check's own lines to the other: both
        // are read to their end, one after the other being fine for output this small.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (first, second) = tokio::join!(
            async {
                if let Some(stream) = stdout {
                    let _ = stream.take(OUTPUT_LIMIT).read_to_end(&mut out).await;
                }
            },
            async {
                if let Some(stream) = stderr {
                    let _ = stream.take(OUTPUT_LIMIT).read_to_end(&mut err).await;
                }
            }
        );
        let ((), ()) = (first, second);
        text.push_str(&String::from_utf8_lossy(&err));
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&out));
        text
    };
    let output = tokio::time::timeout(PATIENCE, read_all)
        .await
        .map_err(|_| Error::Timeout)?;
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    Ok(read(&output))
}

#[cfg(test)]
mod tests {
    use super::*;

    // What Godot 4.7.2 printed for a project with two broken scripts, on 27 November 2026.
    const BROKEN: &str = r#"Godot Engine v4.7.2.stable.official.ed1daf0bf - https://godotengine.org

SCRIPT ERROR: Parse Error: Expected expression for variable initial value after "=".
   at: GDScript::reload (res://broken.gd:3)
   GDScript backtrace (most recent call first):
       [0] _initialize (C:/ghost/game/godot/bridge/check.gd:14)
ERROR: Failed to load script "res://broken.gd" with error "Parse error".
   at: load (modules/gdscript/gdscript_resource_format.cpp:46)
   GDScript backtrace (most recent call first):
       [0] _initialize (C:/ghost/game/godot/bridge/check.gd:14)
GHOST-CHECK-FAILED res://broken.gd
SCRIPT ERROR: Parse Error: Identifier "not_defined_anywhere" not declared in the current scope.
   at: GDScript::reload (res://levels/unknown.gd:3)
GHOST-CHECK-FAILED res://levels/unknown.gd
GHOST-CHECK-DONE 5 2
"#;

    #[test]
    fn each_problem_is_reported_once_with_its_file_and_line() {
        let report = read(BROKEN);
        assert_eq!((report.files, report.failed, report.finished), (5, 2, true));
        assert_eq!(
            report.problems,
            [
                "res://broken.gd:3: Parse Error: Expected expression for variable initial value after \"=\".",
                "res://levels/unknown.gd:3: Parse Error: Identifier \"not_defined_anywhere\" not declared in the current scope.",
            ],
            "the engine's own source files are not the project's"
        );
    }

    #[test]
    fn a_clean_project_and_a_check_that_never_finished_are_told_apart() {
        let clean = read("Godot Engine v4.7.2\n\nGHOST-CHECK-DONE 12 0\n");
        assert_eq!((clean.files, clean.failed, clean.finished), (12, 0, true));
        assert!(clean.problems.is_empty());
        let cut = read("SCRIPT ERROR: Parse Error: bad\n   at: GDScript::reload (res://a.gd:1)\n");
        assert!(!cut.finished);
        assert_eq!(cut.problems, ["res://a.gd:1: Parse Error: bad"]);
        // Whatever the project prints, only Godot's own two-line form is a problem.
        let noise = read("at: (res://fake.gd:1)\nGHOST-CHECK-DONE x y\nERROR: lonely\n");
        assert!(noise.problems.is_empty());
        assert_eq!((noise.files, noise.failed), (0, 0));
    }

    #[test]
    fn the_same_problem_twice_is_listed_once_and_there_is_a_most() {
        let mut output = String::new();
        for number in 0..60 {
            output.push_str(&format!(
                "SCRIPT ERROR: bad\n   at: f (res://a{number}.gd:1)\nSCRIPT ERROR: bad\n   at: f (res://a{number}.gd:1)\n"
            ));
        }
        assert_eq!(read(&output).problems.len(), MAX_PROBLEMS);
    }
}
