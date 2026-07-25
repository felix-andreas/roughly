//! End-to-end tests for the `roughly` binary: diagnostic rendering, JSON
//! output, and the documented exit-code contract (0 clean, 1 findings, 2
//! usage/configuration/IO errors).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

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

// The unclosed `(` produces a syntax error at the 0-based range 2:5..2:6 (the
// `(` token on line 2), so the rendered 1-based position is line 2, column 6.
const SYNTAX_ERROR_SOURCE: &str = "print(1)\ny <- (\n";

//
// CHECK
//

#[test]
fn check_clean_file_exits_zero() {
    let directory = project(&[("clean.R", "x <- 1\nprint(x)\n")]);
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
        stderr.contains("bad.R:2:6"),
        "expected a 1-based `--> path:line:column` header, got: {stderr}"
    );
    assert!(
        stderr.contains("2 | y <- ("),
        "expected a 1-based gutter line number, got: {stderr}"
    );
    assert!(
        !stderr.contains("print(1)"),
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
        .and_then(|line| line.strip_prefix("error[syntax-error]: "))
        .unwrap_or_else(|| panic!("expected an `error[syntax-error]:` line first, got: {stderr}"));
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
        stderr.contains("warning[unused]:"),
        "expected a code-carrying warning diagnostic, got: {stderr}"
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

    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("diagnostic line is not valid JSON"))
        .collect();
    assert!(!records.is_empty(), "expected JSON lines, got: {stdout}");
    let record = &records[0];

    assert!(
        record["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("bad.R")),
        "unexpected path: {record}"
    );
    assert_eq!(record["line"], 2, "line must be 1-based: {record}");
    assert_eq!(record["column"], 6, "column must be 1-based: {record}");
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
    let directory = project(&[("warn.R", "x = 1\nprint(x)\n")]);
    let output = roughly(directory.path(), &["check", "--output", "json", "warn.R"]);

    assert_eq!(exit_code(&output), 1);
    let record: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("diagnostic line is not valid JSON");
    assert_eq!(record["severity"], "warning", "got: {record}");
}

// Two package files overwrite the same top-level binding; each warning
// carries a note pointing at the sibling binding, on both output surfaces.
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

// importFrom naming something a stubbed namespace does not export warns on
// the NAMESPACE file; spelled-right imports and unknown (unstubbed)
// namespaces stay quiet.
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

// The `unused-import` lint is off by default and fires only when opted in; a
// name used anywhere in the package (including via `pkg::name`) is not
// flagged, an unused one is.
#[test]
fn check_flags_unused_imports_when_opted_in() {
    let files: &[(&str, &str)] = &[
        ("R/main.R", "run <- function() stats::sd(c(1, 2))\n"),
        ("NAMESPACE", "importFrom(stats, sd, median)\n"),
    ];

    let default_run = roughly(project(files).path(), &["check", "."]);
    assert_eq!(
        exit_code(&default_run),
        0,
        "stderr: {}",
        stderr(&default_run)
    );

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

// The shadow lints are off by default and fire only when opted in: a
// top-level binding over a `base` name reports `shadows-builtin`, one over
// another stub namespace's name reports `shadows-namespace` naming the
// shadowed symbol.
#[test]
fn check_flags_builtin_shadows_when_opted_in() {
    let files: &[(&str, &str)] = &[("R/main.R", "mean <- function(x) x\nsd <- 1\n")];

    let default_run = roughly(project(files).path(), &["check", "."]);
    assert_eq!(
        exit_code(&default_run),
        0,
        "stderr: {}",
        stderr(&default_run)
    );

    let opted_in = project(&[
        files[0],
        (
            "roughly.toml",
            "[lint]\nshadows-builtin = \"warn\"\nshadows-namespace = \"warn\"\n",
        ),
    ]);
    let output = roughly(opted_in.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("Top-level binding `mean` shadows a builtin."),
        "expected the shadows-builtin warning, got: {rendered}"
    );
    assert!(
        rendered.contains("Top-level binding `sd` shadows `stats::sd`."),
        "expected the shadows-namespace warning, got: {rendered}"
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
        !stderr(&filtered).contains("assignment-operator"),
        "warnings must not render under --min-severity error: {}",
        stderr(&filtered)
    );
    assert!(
        stdout(&filtered).contains("no problems"),
        "the summary counts only what passed the severity floor: {}",
        stdout(&filtered)
    );
}

#[test]
fn check_unknown_config_key_warns_but_starts() {
    let directory = project(&[
        ("clean.R", "x <- 1\nprint(x)\n"),
        ("roughly.toml", "[check]\nstric = true\n"),
    ]);
    let output = roughly(directory.path(), &["check", "clean.R"]);
    assert_eq!(
        exit_code(&output),
        0,
        "an unknown key must not block the run: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("ignoring unknown config key `check.stric`"),
        "the unknown key warns visibly: {}",
        stderr(&output)
    );
}

#[test]
fn check_invalid_config_exits_two() {
    let directory = project(&[
        ("clean.R", "x <- 1\n"),
        ("roughly.toml", "[check]\ntyping = 1\n"),
    ]);

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
    // The loader drops an override declaration it cannot harvest; `check`
    // must say so instead of silently checking against a corpus the author
    // did not write.
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
        ("clean.R", "x <- 1\nprint(x)\n"),
        (
            "stubs/project.Rtypes",
            "my_helper : fn(x: double) -> double\n",
        ),
    ]);
    let output = roughly(directory.path(), &["check", "clean.R"]);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
}

// Every shipped overload set ends in an `Any` catch-all, so the
// no-candidate-accepts error is reachable only through a project override
// whose candidates all constrain; this pins the message.
#[test]
fn check_reports_no_matching_overload() {
    let directory = project(&[
        ("R/main.R", "answer <- pick(\"word\")\n"),
        (
            "stubs/project.Rtypes",
            "pick : fn(x: integer) -> integer\npick : fn(x: double) -> double\n",
        ),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains(
            "no overload of `pick` matches these arguments — I tried all 2 declared signatures"
        ),
        "expected the no-overload finding, got: {rendered}"
    );
}

#[test]
fn check_override_stub_types_apply() {
    let directory = project(&[
        ("R/main.R", "answer <- my_helper(\"not a double\")\n"),
        (
            "stubs/project.Rtypes",
            "my_helper : fn(x: double) -> double\n",
        ),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("R/main.R") || rendered.contains("main.R"),
        "expected a type finding against the override stub, got: {rendered}"
    );
}

#[test]
fn check_namespace_imports_resolve_bare_reads() {
    let unimported = project(&[("R/main.R", "f <- function(x) rbindlist(x)\n")]);
    let output = roughly(unimported.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("could not resolve `rbindlist`"),
        "expected the unresolved warning without an import, got: {rendered}"
    );

    let imported = project(&[
        ("R/main.R", "f <- function(x) rbindlist(x)\n"),
        ("NAMESPACE", "importFrom(data.table, rbindlist)\n"),
    ]);
    let output = roughly(imported.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 0, "stderr: {rendered}");
}

#[test]
fn check_description_dependencies_quiet_namespace_reads() {
    let declared = project(&[
        ("R/main.R", "f <- function(d) dplyr::mutate(d)\n"),
        ("DESCRIPTION", "Package: demo\nImports: dplyr\n"),
    ]);
    let output = roughly(declared.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 0, "stderr: {rendered}");
}

#[test]
fn check_collate_order_decides_the_package_winner() {
    // Both files bind `value`; the reader errors only when the character
    // binding wins. Path order makes `b.R` the later writer; the Collate
    // order reverses that, so the integer binding wins and the reader is
    // clean. Duplicate-binding warnings fire either way — the error
    // severity floor keeps the assertion on the type finding alone.
    let base: &[(&str, &str)] = &[
        ("R/a.R", "value <- 1L\n"),
        ("R/b.R", "value <- \"word\"\n"),
        ("R/use.R", "f <- function() value + 1L\n"),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ];
    let path_ordered = project(base);
    let output = roughly(
        path_ordered.path(),
        &["check", ".", "--min-severity", "error"],
    );
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("expected a numeric value"),
        "expected the reader to see the character winner, got: {rendered}"
    );

    let mut with_collate = base.to_vec();
    with_collate.push(("DESCRIPTION", "Package: demo\nCollate: 'b.R' 'a.R'\n"));
    let collated = project(&with_collate);
    let output = roughly(collated.path(), &["check", ".", "--min-severity", "error"]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 0, "stderr: {rendered}");
}

#[test]
fn analysis_stats_reports_phases_and_probe() {
    let directory = project(&[
        ("R/a.R", "value <- 1L\nf <- function() value + 1L\n"),
        ("R/b.R", "g <- function(x) f() + x\n"),
    ]);
    let output = roughly(directory.path(), &["debug", "analysis-stats", "."]);
    let rendered = stdout(&output);
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
    for section in [
        "cold analysis",
        "typecheck (+interfaces)",
        "slowest files (typecheck):",
        "incremental (typing burst",
    ] {
        assert!(
            rendered.contains(section),
            "expected `{section}` in the report, got: {rendered}"
        );
    }
}

#[test]
fn check_without_r_files_reports_nothing_and_exits_clean() {
    let directory = project(&[]);
    let output = roughly(directory.path(), &["check"]);
    // A tree with no R in it yet is not a usage error: failing here would fail
    // a pipeline over a stage that simply has nothing to check.
    assert_eq!(exit_code(&output), 0, "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("0 files checked, no problems"),
        "expected the empty-target summary, got: {}",
        stdout(&output)
    );
}

#[test]
fn check_answer_is_independent_of_how_the_paths_are_named() {
    // A `#: @alias` declared in one package file, referenced from another: the
    // reference must resolve however the command spells the paths, or
    // `check R/` and `check $(git diff --name-only)` cannot gate a pipeline.
    let directory = project(&[
        ("roughly.toml", "[check]\ntyping = true\n"),
        (
            "R/types.R",
            "#: @alias Config {list{id: character}}\nNULL\n",
        ),
        (
            "R/build.R",
            "#: fn(id: character) -> Config\nbuild <- function(id) list(id = id)\n",
        ),
    ]);
    for arguments in [
        vec!["check", "."],
        vec!["check", "R"],
        vec!["check", "R/build.R"],
        vec!["check", "R/build.R", "R/types.R"],
    ] {
        let output = roughly(directory.path(), &arguments);
        assert_eq!(
            exit_code(&output),
            0,
            "`{}` must agree with every other spelling: {}",
            arguments.join(" "),
            stderr(&output)
        );
    }
}

#[test]
fn check_reports_an_export_the_package_does_not_define() {
    let directory = project(&[
        ("DESCRIPTION", "Package: expkg\n"),
        (
            "NAMESPACE",
            "export(add_one)\nexport(missing_fn)\nexportPattern(\"^get_\")\n",
        ),
        (
            "R/a.R",
            "add_one <- function(x) x + 1L\nget_thing <- function() 1L\n",
        ),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert!(
        rendered.contains(
            "`missing_fn` is exported but this package defines no such top-level object."
        ) && !rendered.contains("add_one` is exported")
            && !rendered.contains("get_thing` is exported"),
        "only the undefined export is reported: {rendered}"
    );
}

#[test]
fn check_roots_a_nested_package_at_its_own_description() {
    // An ancestor `roughly.toml` must not swallow a package in a
    // subdirectory: that package's own DESCRIPTION is nearer, so its `R/`
    // files are package source and their top-level bindings are not unused.
    let directory = project(&[
        ("roughly.toml", "[check]\ntyping = true\n"),
        ("pkg/DESCRIPTION", "Package: inner\n"),
        ("pkg/R/a.R", "helper <- function() 1L\n"),
    ]);
    let output = roughly(directory.path(), &["check", "pkg"]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 0, "stderr: {rendered}");
    assert!(
        !rendered.contains("unused"),
        "a package-visible binding is not unused: {rendered}"
    );
}

#[test]
fn check_reports_only_the_paths_it_was_given() {
    let directory = project(&[
        ("roughly.toml", "[check]\ntyping = true\n"),
        ("R/inside.R", "package_bad <- 1L + \"x\"\n"),
        ("driver.R", "script_bad <- 2L + \"y\"\n"),
    ]);
    let output = roughly(directory.path(), &["check", "R"]);
    let rendered = stderr(&output);
    assert!(
        rendered.contains("R/inside.R") && !rendered.contains("driver.R"),
        "analysis covers the project, reporting covers the request: {rendered}"
    );
}

#[test]
fn check_analyses_r_chunks_in_a_literate_document() {
    let directory = project(&[
        ("roughly.toml", "[check]\ntyping = true\n"),
        (
            "report.Rmd",
            "# Title\n\nProse about it.\n\n```{r}\ncount <- 10L\n```\n\n```{r}\nprint(count + \"text\")\n```\n\n```{python}\nx = 1\n```\n",
        ),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("report.Rmd:10:") && rendered.contains("found `character`"),
        "the chunk error is located in the original document: {rendered}"
    );
    // The formatter must leave a literate document alone.
    let formatted = roughly(directory.path(), &["fmt", "--check", "."]);
    assert!(
        !stdout(&formatted).contains("1 file"),
        "fmt must not pick up literate documents: {}",
        stdout(&formatted)
    );
}

#[test]
fn check_exclude_scopes_the_directory_walk() {
    let directory = project(&[
        ("roughly.toml", "[check]\nexclude = [\"scripts/\"]\n"),
        ("top.R", "top_unused <- 1\n"),
        ("scripts/skipped.R", "script_unused <- 2\n"),
        ("scripts/deep/also.R", "deep_unused <- 3\n"),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("top_unused"),
        "the non-excluded script is checked: {rendered}"
    );
    assert!(
        !rendered.contains("script_unused") && !rendered.contains("deep_unused"),
        "the excluded tree must not be walked: {rendered}"
    );
}

#[test]
fn check_exclude_negation_reincludes() {
    let directory = project(&[
        (
            "roughly.toml",
            "[check]\nexclude = [\"scripts/*\", \"!scripts/keep\"]\n",
        ),
        ("scripts/skipped.R", "script_unused <- 2\n"),
        ("scripts/keep/kept.R", "kept_unused <- 3\n"),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("kept_unused") && !rendered.contains("script_unused"),
        "negation re-includes the kept subtree: {rendered}"
    );
}

#[test]
fn check_explicit_file_bypasses_exclude() {
    let directory = project(&[
        ("roughly.toml", "[check]\nexclude = [\"scripts/\"]\n"),
        ("scripts/named.R", "named_unused <- 2\n"),
    ]);
    let output = roughly(directory.path(), &["check", "scripts/named.R"]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("named_unused"),
        "a file named on the command line is always checked: {rendered}"
    );
}

#[test]
fn check_invalid_exclude_pattern_exits_two() {
    let directory = project(&[
        ("roughly.toml", "[check]\nexclude = [\"scripts/[\"]\n"),
        ("top.R", "x <- 1\n"),
    ]);
    let output = roughly(directory.path(), &["check", "."]);
    assert_eq!(exit_code(&output), 2, "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains("invalid `[check] exclude` pattern"),
        "the bad pattern is named: {}",
        stderr(&output)
    );
}

// A suppression applies to its own line and the line directly below it, so
// the unsuppressed finding needs a blank line in between.
#[test]
fn check_suppression_comments_drop_findings() {
    let directory = project(&[(
        "warn.R",
        "x = 1 # roughly: allow(assignment-operator, unused)\n\ny = 2\n",
    )]);
    let output = roughly(directory.path(), &["check", "warn.R"]);
    let rendered = stderr(&output);
    assert_eq!(exit_code(&output), 1, "stderr: {rendered}");
    assert!(
        rendered.contains("warn.R:3:1"),
        "the unsuppressed finding remains: {rendered}"
    );
    assert!(
        !rendered.contains("warn.R:1:1"),
        "the suppressed finding must not render: {rendered}"
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
// PER-FILE TYPING DIRECTIVES
//

// `# typing: off` silences type errors for its file even when the
// configuration checks types; files without a directive keep the configured
// behavior.
#[test]
fn typing_off_directive_silences_one_file() {
    let directory = project(&[
        ("R/opted_out.R", "# typing: off\nbad <- 1L + \"a\"\n"),
        ("R/plain.R", "also_bad <- 1L + \"a\"\n"),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    let report = stderr(&output);
    assert!(report.contains("plain.R"), "report:\n{report}");
    assert!(!report.contains("opted_out.R"), "report:\n{report}");
}

// `# typing: on` opts a single file into type checking when the
// configuration has it off, and `# typing: strict` additionally escalates
// unresolved references — both without touching the rest of the workspace.
#[test]
fn typing_on_and_strict_directives_opt_single_files_in() {
    let directory = project(&[
        ("R/opted_in.R", "# typing: on\nbad <- 1L + \"a\"\n"),
        ("R/strict_file.R", "# typing: strict\nx <- not_defined()\n"),
        ("R/plain.R", "quiet <- 1L + \"a\"\n"),
        ("roughly.toml", "[check]\ntyping = false\n"),
    ]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    let report = stderr(&output);
    assert!(report.contains("opted_in.R"), "report:\n{report}");
    assert!(
        report.contains("error[unresolved]: I could not resolve `not_defined`"),
        "strict escalates the unresolved reference to an error:\n{report}"
    );
    assert!(!report.contains("plain.R"), "report:\n{report}");
}

// A typo'd directive value is a diagnostic, not a silent no-op.
#[test]
fn unknown_typing_directive_value_is_reported() {
    let directory = project(&[("R/a.R", "# typing: strcit\nx <- 1L\n")]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("Unknown typing directive `strcit`"),
        "report:\n{}",
        stderr(&output)
    );
}

// data.table's non-standard evaluation: bare column references inside a
// bracket carrying the data.table signature and inside the base
// `with`/`subset` family resolve as data-masked columns — no
// unresolved-name warnings — while base indexing keeps them.
#[test]
fn data_masked_column_references_do_not_warn() {
    let directory = project(&[
        (
            "R/dt.R",
            "summarize_sales <- function(dt) {\n  dt[region == \"west\", .(total = sum(amount)), by = product]\n  dt[, revenue := price * quantity]\n  with(dt, mean(score_col))\n}\n",
        ),
        ("R/plain.R", "pick <- function(m) m[not_a_column]\n"),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    let report = stderr(&output);
    assert!(
        report.contains("not_a_column"),
        "base indexing keeps its unresolved warning:\n{report}"
    );
    for column in ["region", "amount", "product", "revenue", "score_col"] {
        assert!(
            !report.contains(&format!("`{column}`")),
            "masked column `{column}` must not warn:\n{report}"
        );
    }
}

// `utils::globalVariables(c(...))` — the ecosystem-standard escape hatch —
// declares names as dynamically bound for the whole package:
// could-not-resolve is suppressed for them everywhere, while undeclared
// names keep warning.
#[test]
fn global_variables_declarations_suppress_unresolved_warnings() {
    let directory = project(&[
        (
            "R/globals.R",
            "utils::globalVariables(c(\"generated_col\", \"another_col\"))\n",
        ),
        (
            "R/use.R",
            "f <- function() generated_col + another_col + genuinely_undefined\n",
        ),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    let report = stderr(&output);
    assert!(report.contains("genuinely_undefined"), "report:\n{report}");
    for declared in ["generated_col", "another_col"] {
        assert!(
            !report.contains(&format!("`{declared}`")),
            "declared name `{declared}` must not warn:\n{report}"
        );
    }
}

// A project stub can declare a dplyr-style verb `@masked`: the arguments its
// `...` rest parameter absorbs evaluate in the data's frame, so bare column
// references there stop warning — for the bare and the `pkg::name` call
// forms alike — while a locally shadowed name masks nothing.
#[test]
fn masked_stub_verbs_mask_rest_absorbed_arguments() {
    let directory = project(&[
        (
            "stubs/dplyr.Rtypes",
            "filter : @masked fn(.data: Any, ...: Any) -> Any\nmutate : @masked fn(.data: Any, ...: Any) -> Any\n",
        ),
        (
            "R/pipeline.R",
            "report <- function(df) {\n  filtered <- dplyr::filter(df, amount > 100)\n  mutate(filtered, doubled = amount * 2)\n}\ncontrol <- function() {\n  filter <- function(x) x\n  filter(shadowed_undefined)\n}\n",
        ),
        ("roughly.toml", "[check]\ntyping = true\n"),
    ]);
    let output = roughly(directory.path(), &["check"]);
    assert_eq!(exit_code(&output), 1);
    let report = stderr(&output);
    assert!(report.contains("shadowed_undefined"), "report:\n{report}");
    for column in ["amount", "doubled"] {
        assert!(
            !report.contains(&format!("`{column}`")),
            "masked column `{column}` must not warn:\n{report}"
        );
    }
}
