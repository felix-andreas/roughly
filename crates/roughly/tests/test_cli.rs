//! End-to-end tests for the `roughly` binary: diagnostic rendering, JSON output, and the
//! documented exit-code contract (0 clean, 1 findings, 2 usage/configuration/IO errors).

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn roughly(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_roughly"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("failed to run the roughly binary")
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().expect("terminated by signal")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("failed to create a temporary directory");
    for (name, content) in files {
        let path = directory.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create test directories");
        }
        fs::write(path, content).expect("failed to write a test file");
    }
    directory
}

// The source produces one syntax-error diagnostic with the 0-based range 1:2..1:6, so the
// rendered 1-based position is line 2, column 3.
const SYNTAX_ERROR_SOURCE: &str = "x <- 1\ny <- (\n";

//
// CHECK
//

#[test]
fn check_clean_file_exits_zero() {
    let directory = project(&[("clean.R", "x <- 1\n")]);
    let output = roughly(directory.path(), &["check", "clean.R"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
}

#[test]
fn check_renders_one_based_positions() {
    let directory = project(&[("bad.R", SYNTAX_ERROR_SOURCE)]);
    let output = roughly(directory.path(), &["check", "bad.R"]);
    let stderr = stderr(&output);

    assert_eq!(exit_code(&output), 1, "stderr: {stderr}");
    assert!(
        stderr.contains("bad.R:2:3"),
        "expected a 1-based `--> path:line:column` header, got: {stderr}"
    );
    assert!(
        stderr.contains("2 | y <- ("),
        "expected a 1-based gutter line number, got: {stderr}"
    );
    assert!(
        !stderr.contains("x <- 1"),
        "expected only the diagnostic's own line in the snippet, got: {stderr}"
    );
}

#[test]
fn check_does_not_repeat_the_message_under_the_caret() {
    let directory = project(&[("bad.R", SYNTAX_ERROR_SOURCE)]);
    let output = roughly(directory.path(), &["check", "bad.R"]);
    let stderr = stderr(&output);

    let message = stderr
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("error: "))
        .unwrap_or_else(|| panic!("expected an `error:` line first, got: {stderr}"));
    assert_eq!(
        stderr.matches(message).count(),
        1,
        "expected the message to appear exactly once, got: {stderr}"
    );
    assert!(
        stderr.contains('^'),
        "expected a caret underline, got: {stderr}"
    );
}

#[test]
fn check_counts_warnings_as_findings() {
    let directory = project(&[("warn.R", "x = 1\n")]);
    let output = roughly(directory.path(), &["check", "warn.R"]);
    let stderr = stderr(&output);

    assert!(
        stderr.contains("warning:"),
        "expected a warning diagnostic, got: {stderr}"
    );
    assert_eq!(exit_code(&output), 1, "warnings must exit 1: {stderr}");
}

#[test]
fn check_json_output_is_one_based_and_parses() {
    let directory = project(&[("bad.R", SYNTAX_ERROR_SOURCE)]);
    let output = roughly(directory.path(), &["check", "--output", "json", "bad.R"]);
    let stdout = stdout(&output);

    assert_eq!(exit_code(&output), 1, "stderr: {}", stderr(&output));
    assert!(
        !stderr(&output).contains("-->"),
        "expected no human rendering in json mode, got: {}",
        stderr(&output)
    );

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected one JSON line, got: {stdout}");
    let record: serde_json::Value =
        serde_json::from_str(lines[0]).expect("diagnostic line is not valid JSON");

    assert!(
        record["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("bad.R")),
        "unexpected path: {record}"
    );
    assert_eq!(record["line"], 2, "line must be 1-based: {record}");
    assert_eq!(record["column"], 3, "column must be 1-based: {record}");
    assert_eq!(record["endLine"], 2, "endLine must be 1-based: {record}");
    assert_eq!(
        record["endColumn"], 7,
        "endColumn must be 1-based: {record}"
    );
    assert_eq!(record["severity"], "error", "unexpected severity: {record}");
    assert!(
        record["code"].as_str().is_some_and(|code| !code.is_empty()),
        "expected a diagnostic code: {record}"
    );
    assert!(
        record["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "expected a message: {record}"
    );
}

#[test]
fn check_json_output_maps_warning_severity() {
    let directory = project(&[("warn.R", "x = 1\n")]);
    let output = roughly(directory.path(), &["check", "--output", "json", "warn.R"]);

    assert_eq!(exit_code(&output), 1);
    let record: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("diagnostic line is not valid JSON");
    assert_eq!(record["severity"], "warning", "got: {record}");
}

// Two package files overwrite the same top-level binding; each warning carries a note pointing at
// the sibling binding, on both output surfaces.
#[test]
fn check_related_notes_render_on_both_surfaces() {
    let files = &[("R/a.R", "shared <- 1\n"), ("R/b.R", "shared <- 2\n")];

    let human = roughly(project(files).path(), &["check", "."]);
    assert_eq!(exit_code(&human), 1, "stderr: {}", stderr(&human));
    let rendered = stderr(&human);
    assert!(
        rendered.contains("= note: the later binding is here. -->")
            && rendered.contains("= note: the earlier binding is here. -->"),
        "expected one note per overwrite warning, got: {rendered}"
    );

    let json = roughly(project(files).path(), &["check", "--output", "json", "."]);
    assert_eq!(exit_code(&json), 1, "stderr: {}", stderr(&json));
    let records: Vec<serde_json::Value> = stdout(&json)
        .lines()
        .map(|line| serde_json::from_str(line).expect("diagnostic line is not valid JSON"))
        .collect();
    let noted = records
        .iter()
        .filter_map(|record| record["related"].as_array())
        .filter(|related| !related.is_empty())
        .count();
    assert_eq!(
        noted, 2,
        "expected both overwrite warnings to carry related info: {records:?}"
    );
    let entry = records
        .iter()
        .find_map(|record| record["related"].as_array().and_then(|array| array.first()))
        .expect("a related entry exists");
    assert!(
        entry["path"]
            .as_str()
            .is_some_and(|path| path.ends_with(".R")),
        "related path points at the sibling file: {entry}"
    );
    assert_eq!(entry["line"], 1, "related line must be 1-based: {entry}");
    assert_eq!(
        entry["column"], 1,
        "related column must be 1-based: {entry}"
    );
    assert!(
        entry["message"]
            .as_str()
            .is_some_and(|message| message.contains("binding is here")),
        "related message names the sibling binding: {entry}"
    );
}

// importFrom naming something a stubbed namespace does not export warns on the NAMESPACE file;
// spelled-right imports and unknown (unstubbed) namespaces stay quiet.
#[test]
fn check_validates_namespace_imports_against_stubs() {
    let directory = project(&[
        ("R/main.R", "x <- 1\n"),
        (
            "NAMESPACE",
            "import(stats)\nimportFrom(stats, sd, medain)\nimportFrom(dplyr, mutate)\n",
        ),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("`medain` is not exported by `stats`."),
        "expected the typo warning, got: {rendered}"
    );
    assert!(
        !rendered.contains("mutate") && !rendered.contains("`sd`"),
        "unknown namespaces and real exports must stay quiet: {rendered}"
    );
    assert!(
        rendered.contains("NAMESPACE:2:23"),
        "expected a precise range on the NAMESPACE file, got: {rendered}"
    );
}

// The `unused-import` lint is off by default and fires only when opted in; a name used anywhere in
// the package (including via `pkg::name`) is not flagged, an unused one is.
#[test]
fn check_flags_unused_imports_when_opted_in() {
    let files: &[(&str, &str)] = &[
        ("R/main.R", "run <- function() stats::sd(c(1, 2))\n"),
        ("NAMESPACE", "importFrom(stats, sd, median)\n"),
    ];

    // Default: silent.
    let default_run = roughly(project(files).path(), &["check", "."]);
    assert_eq!(
        exit_code(&default_run),
        0,
        "stderr: {}",
        stderr(&default_run)
    );

    // Opted in: `median` is flagged, `sd` (used via stats::sd) is not.
    let opted_in = project(&[
        files[0],
        files[1],
        ("roughly.toml", "[lint]\nunused-import = \"warn\"\n"),
    ]);
    let output = roughly(opted_in.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("imported name `median` from `stats` is never used."),
        "expected the unused-import warning, got: {rendered}"
    );
    assert!(
        !rendered.contains("`sd`"),
        "a name used via pkg::name must not be flagged: {rendered}"
    );
}

#[test]
fn check_min_severity_error_ignores_warnings() {
    let directory = project(&[("warn.R", "x = 1\n")]);

    let filtered = roughly(
        directory.path(),
        &["check", "--min-severity", "error", "warn.R"],
    );
    assert_eq!(exit_code(&filtered), 0, "stderr: {}", stderr(&filtered));
    assert!(
        stdout(&filtered).trim().is_empty(),
        "warnings must not render under --min-severity error: {}",
        stdout(&filtered)
    );
}

#[test]
fn check_invalid_config_exits_two() {
    let directory = project(&[("clean.R", "x <- 1\n"), ("roughly.toml", "debug = 1\n")]);

    let check = roughly(directory.path(), &["check", "clean.R"]);
    assert_eq!(exit_code(&check), 2, "stderr: {}", stderr(&check));
    assert!(
        stderr(&check).contains("invalid config"),
        "expected the config error to be reported, got: {}",
        stderr(&check)
    );

    let fmt = roughly(directory.path(), &["fmt", "--check", "clean.R"]);
    assert_eq!(exit_code(&fmt), 2, "stderr: {}", stderr(&fmt));
}

#[test]
fn check_missing_target_exits_two() {
    let directory = project(&[]);
    let output = roughly(directory.path(), &["check", "does-not-exist.R"]);
    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr(&output));
}

#[test]
fn check_reports_dropped_override_stub_declarations() {
    // The loader drops an override declaration it cannot harvest; `check` must say so instead of
    // silently checking against a corpus the author did not write.
    let directory = project(&[
        ("clean.R", "x <- 1\n"),
        ("stubs/project.Rtypes", "size : Frobnicate\n"),
    ]);
    let output = roughly(directory.path(), &["check", "clean.R"]);
    let stderr = stderr(&output);

    assert_eq!(
        exit_code(&output),
        1,
        "a dropped override declaration is a finding: {stderr}"
    );
    assert!(
        stderr.contains("does not load"),
        "expected the dropped declaration to be reported, got: {stderr}"
    );
    assert!(
        stderr.contains("project.Rtypes:1:1"),
        "expected a 1-based stub-file position header, got: {stderr}"
    );
}

#[test]
fn check_loads_valid_override_stubs_silently() {
    let directory = project(&[
        ("clean.R", "x <- 1\n"),
        (
            "stubs/project.Rtypes",
            "my_helper : fn(x: double) -> double\n",
        ),
    ]);
    let output = roughly(directory.path(), &["check", "clean.R"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
}

#[test]
fn check_without_r_files_exits_two() {
    let directory = project(&[]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("no R files found"),
        "expected the empty-target error, got: {}",
        stderr(&output)
    );
}

//
// FMT
//

#[test]
fn fmt_check_lists_dirty_files_and_writes_nothing() {
    let directory = project(&[("dirty.R", "x<-1\n"), ("clean.R", "y <- 2\n")]);
    let output = roughly(directory.path(), &["fmt", "--check"]);
    let stderr = stderr(&output);

    assert_eq!(exit_code(&output), 1, "stderr: {stderr}");
    assert!(
        stderr.contains("Would reformat") && stderr.contains("dirty.R"),
        "expected the dirty file to be listed, got: {stderr}"
    );
    assert!(
        stderr.contains("1 file would be reformatted, 1 file already formatted"),
        "expected a summary count, got: {stderr}"
    );
    let content = fs::read_to_string(directory.path().join("dirty.R")).expect("read dirty.R");
    assert_eq!(content, "x<-1\n", "--check must not modify files");
}

#[test]
fn fmt_check_clean_exits_zero_with_summary() {
    let directory = project(&[("clean.R", "x <- 1\n")]);
    let output = roughly(directory.path(), &["fmt", "--check", "clean.R"]);
    let stderr = stderr(&output);

    assert_eq!(exit_code(&output), 0, "stderr: {stderr}");
    assert!(
        stderr.contains("0 files would be reformatted, 1 file already formatted"),
        "expected a summary line, got: {stderr}"
    );
}

#[test]
fn fmt_rewrites_in_place_and_exits_zero() {
    let directory = project(&[("dirty.R", "x<-1\n")]);
    let output = roughly(directory.path(), &["fmt", "dirty.R"]);

    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
    let content = fs::read_to_string(directory.path().join("dirty.R")).expect("read dirty.R");
    assert_eq!(content, "x <- 1\n");
}

#[test]
fn fmt_syntax_error_exits_two() {
    let directory = project(&[("bad.R", SYNTAX_ERROR_SOURCE)]);
    let output = roughly(directory.path(), &["fmt", "bad.R"]);

    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("failed to format"),
        "expected the format failure to be reported, got: {}",
        stderr(&output)
    );
}

//
// USAGE
//

#[test]
fn usage_error_exits_two() {
    let directory = project(&[]);
    let output = roughly(directory.path(), &["bogus-subcommand"]);
    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr(&output));
}

//
// ANALYSIS-STATS
//

// The performance diagnosis runs the full pipeline and prints every report section; the workspace
// here has a cross-file reference so the interface layer genuinely participates.
#[test]
fn analysis_stats_prints_the_full_breakdown() {
    let directory = project(&[
        ("R/lib.R", "make_count <- function() 1L\n"),
        ("R/use.R", "total <- make_count() + 1L\n"),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["analysis-stats"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
    let report = stdout(&output);
    for section in [
        "workspace:",
        "2 package files",
        "cold analysis",
        "typecheck (+interfaces)",
        "slowest files (typecheck):",
        "incremental (body edit on",
        "edited-file diagnostics",
        "workspace revalidate",
    ] {
        assert!(
            report.contains(section),
            "missing section {section:?} in report:\n{report}"
        );
    }
    assert!(
        !report.contains("note: [check] typing is off"),
        "typing is on in this workspace's config, so no forcing note:\n{report}"
    );
}

// Without a configuration the type checker defaults off; the diagnosis forces it on and says so.
#[test]
fn analysis_stats_notes_when_it_forces_typing_on() {
    let directory = project(&[("R/a.R", "f <- function(x) x + 1\n")]);
    let output = roughly(directory.path(), &["analysis-stats"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("note: [check] typing is off in the configuration"),
        "report:\n{}",
        stdout(&output)
    );
}

// A target without any R files is a usage error (exit 2), not an empty report.
#[test]
fn analysis_stats_rejects_a_workspace_without_r_files() {
    let directory = project(&[("README.md", "no code here\n")]);
    let output = roughly(directory.path(), &["analysis-stats"]);
    assert_eq!(exit_code(&output), 2);
    assert!(stderr(&output).contains("no R files"));
}
