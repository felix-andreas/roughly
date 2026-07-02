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
        fs::write(directory.path().join(name), content).expect("failed to write a test file");
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
