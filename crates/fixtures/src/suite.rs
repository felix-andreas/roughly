use {
    crate::{
        Fixture, FixtureExpectedOutput, FixtureGenerationEntry, FixtureGroup, FixtureKind,
        FixtureOperation, MultiFileFixture, expectation_content_spans, parse_file,
    },
    std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        ops::Range,
        path::{Path, PathBuf},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureRunFile {
    pub path: PathBuf,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureExpectedPaths {
    pub name: String,
    pub paths: Vec<PathBuf>,
}

pub fn run_fixture_suite<RunFixture>(directory_path: &str, run_fixture: RunFixture)
where
    RunFixture: FnMut(&Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String>,
{
    run_fixture_suite_with_bless(directory_path, run_fixture, bless_enabled());
}

// Tests force the bless flag here rather than through `ROUGHLY_BLESS`: the suite tests run as
// parallel threads in one binary, so a process-wide environment variable would leak into unrelated
// fixture runs.
pub fn run_fixture_suite_with_bless<RunFixture>(
    directory_path: &str,
    mut run_fixture: RunFixture,
    bless: bool,
) where
    RunFixture: FnMut(&Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String>,
{
    let groups = read_fixture_suite(directory_path);
    let maybe_filter = std::env::var("FIXTURE_FILTER").ok();
    let mut bless_state = bless.then(|| build_bless_state(directory_path));
    let mut snapshot_names = BTreeSet::new();
    let mut failures = Vec::new();
    let mut executed_fixture_count = 0;

    for group in &groups {
        for fixture in &group.cases {
            let fixture_name = format!("{}__{}", group.name, fixture.name);

            assert!(
                snapshot_names.insert(fixture_name.clone()),
                "duplicate fixture snapshot name `{fixture_name}`"
            );

            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !fixture_name.contains(filter))
            {
                continue;
            }

            executed_fixture_count += 1;

            let expected_outputs = match expected_outputs_for_fixture(fixture) {
                Ok(expected_outputs) => expected_outputs,
                Err(error) => {
                    failures.push(format!(
                        "fixture `{fixture_name}` is invalid for the current runner: {error}"
                    ));
                    continue;
                }
            };

            let actual_outputs = match run_fixture(fixture) {
                Ok(actual_outputs) => actual_outputs,
                Err(error) => {
                    failures.push(format!(
                        "fixture `{fixture_name}` is not supported by the current runner: {error}"
                    ));
                    continue;
                }
            };

            if let Some(bless_state) = bless_state.as_mut() {
                bless_or_record_failure(
                    bless_state,
                    fixture,
                    &fixture_name,
                    &expected_outputs,
                    &actual_outputs,
                    &mut failures,
                );
                continue;
            }

            if let Some(failure) =
                compare_fixture_outputs(fixture, &fixture_name, &expected_outputs, &actual_outputs)
            {
                failures.push(failure);
            }
        }
    }

    if let Some(bless_state) = bless_state {
        apply_bless_rewrites(bless_state);
    }

    if !failures.is_empty() {
        panic!(
            "{} fixture test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!("{} fixture(s) passed", executed_fixture_count);
}

fn bless_enabled() -> bool {
    matches!(std::env::var("ROUGHLY_BLESS").as_deref(), Ok("1"))
}

// Schedules content rewrites for a fixture's drifted `#++++` blocks. When the fixture is not
// structurally blessable (snapshot count, missing/extra paths, or duplicate outputs), bless cannot
// repair it, so it falls back to recording the normal rich-diff failure instead.
fn bless_or_record_failure(
    bless_state: &mut BlessState,
    fixture: &Fixture,
    fixture_name: &str,
    expected_outputs: &[ExpectedFixtureOutput],
    actual_outputs: &[Vec<FixtureRunFile>],
    failures: &mut Vec<String>,
) {
    let target = bless_state.targets.get(fixture_name).unwrap_or_else(|| {
        panic!("missing bless target for fixture `{fixture_name}`; span capture is out of sync")
    });
    let source_path = target.source_path.clone();
    let spans = target.spans.clone();
    let source_text = bless_state
        .sources
        .get(&source_path)
        .expect("bless source text should be captured")
        .clone();
    let locations = case_expectations_in_order(fixture);

    match plan_bless_rewrites(
        expected_outputs,
        actual_outputs,
        &locations,
        &spans,
        &source_text,
    ) {
        Some(rewrites) => {
            if !rewrites.is_empty() {
                bless_state
                    .rewrites
                    .entry(source_path)
                    .or_default()
                    .extend(rewrites);
            }
        }
        None => {
            if let Some(failure) =
                compare_fixture_outputs(fixture, fixture_name, expected_outputs, actual_outputs)
            {
                failures.push(failure);
            }
        }
    }
}

// Computes the content rewrites needed to bless a fixture's `#++++` blocks, or `None` when the
// fixture is not structurally blessable (snapshot count, missing/extra paths, or duplicate outputs)
// and should instead be reported with the normal rich diff. Only drifted `Exact` blocks are
// rewritten; `#++++ any` blocks are left untouched.
fn plan_bless_rewrites(
    expected_outputs: &[ExpectedFixtureOutput],
    actual_outputs: &[Vec<FixtureRunFile>],
    locations: &[ExpectationLocation],
    spans: &[Range<usize>],
    source_text: &str,
) -> Option<Vec<(Range<usize>, String)>> {
    if actual_outputs.len() != expected_outputs.len() || locations.len() != spans.len() {
        return None;
    }

    let mut actual_snapshots = Vec::with_capacity(actual_outputs.len());
    for snapshot in actual_outputs {
        let mut files = BTreeMap::new();
        for file in snapshot {
            if files
                .insert(file.path.clone(), file.output.trim_end().to_owned())
                .is_some()
            {
                return None;
            }
        }
        actual_snapshots.push(files);
    }

    for (expected_output, actual_files) in expected_outputs.iter().zip(&actual_snapshots) {
        for path in actual_files.keys() {
            if !expected_output.files.contains_key(path) {
                return None;
            }
        }
        for path in expected_output.files.keys() {
            if !actual_files.contains_key(path) {
                return None;
            }
        }
    }

    let mut rewrites = Vec::new();
    for (location, span) in locations.iter().zip(spans) {
        let Some(expected_text) = &location.expected else {
            continue;
        };
        let actual_text = actual_snapshots
            .get(location.generation_index)
            .and_then(|files| files.get(&location.path))?;
        if actual_text != expected_text {
            rewrites.push((
                span.clone(),
                blessed_block_body(source_text, span, actual_text),
            ));
        }
    }

    Some(rewrites)
}

// Builds the replacement bytes for one expectation block. The trailing whitespace of the existing
// block (the blank-line spacing before the next directive) is preserved so a blessed file matches
// hand-written formatting and re-running without bless is byte-stable. An empty block has no
// trailing newline, so one is added to keep the new content off the following directive line.
fn blessed_block_body(source_text: &str, span: &Range<usize>, actual_text: &str) -> String {
    let original_body = &source_text[span.clone()];
    let trailing_whitespace = &original_body[original_body.trim_end().len()..];
    if trailing_whitespace.contains('\n') {
        format!("{actual_text}{trailing_whitespace}")
    } else {
        format!("{actual_text}{trailing_whitespace}\n")
    }
}

fn case_expectations_in_order(fixture: &Fixture) -> Vec<ExpectationLocation> {
    match &fixture.kind {
        FixtureKind::Simple(case) => vec![ExpectationLocation {
            generation_index: 0,
            path: PathBuf::new(),
            expected: Some(case.expected.clone()),
        }],
        FixtureKind::MultiFile(case) => {
            let mut locations = Vec::new();
            push_generation_expectations(&mut locations, 0, &case.initial_generation.entries);
            for (generation_offset, generation) in case.generations.iter().enumerate() {
                push_generation_expectations(
                    &mut locations,
                    generation_offset + 1,
                    &generation.entries,
                );
            }
            locations
        }
    }
}

fn push_generation_expectations(
    locations: &mut Vec<ExpectationLocation>,
    generation_index: usize,
    entries: &[FixtureGenerationEntry],
) {
    for entry in entries {
        let Some(expectation) = &entry.expectation else {
            continue;
        };
        locations.push(ExpectationLocation {
            generation_index,
            path: expectation.path.clone(),
            expected: match &expectation.output {
                FixtureExpectedOutput::Exact(text) => Some(text.clone()),
                FixtureExpectedOutput::Any => None,
            },
        });
    }
}

fn build_bless_state(directory_path: &str) -> BlessState {
    let mut targets = BTreeMap::new();
    let mut sources = BTreeMap::new();

    for fixture_path in collect_fixture_paths(directory_path) {
        let source_text = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "failed to read fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        let fixture_file = parse_file(&source_text).unwrap_or_else(|error| {
            panic!(
                "failed to parse fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        let spans = expectation_content_spans(&source_text);

        let mut span_index = 0;
        for group in &fixture_file.groups {
            for fixture in &group.cases {
                let expectation_count = case_expectations_in_order(fixture).len();
                let span_end = span_index + expectation_count;
                if span_end > spans.len() {
                    panic!(
                        "bless span capture out of sync in `{}`: not enough `#++++` blocks",
                        fixture_path.display()
                    );
                }
                let case_spans = spans[span_index..span_end].to_vec();
                span_index = span_end;
                let fixture_name = format!("{}__{}", group.name, fixture.name);
                targets.insert(
                    fixture_name,
                    BlessTarget {
                        source_path: fixture_path.clone(),
                        spans: case_spans,
                    },
                );
            }
        }

        if span_index != spans.len() {
            panic!(
                "bless span capture out of sync in `{}`: {} `#++++` blocks, {} consumed",
                fixture_path.display(),
                spans.len(),
                span_index
            );
        }

        sources.insert(fixture_path, source_text);
    }

    BlessState {
        targets,
        sources,
        rewrites: BTreeMap::new(),
    }
}

fn apply_bless_rewrites(bless_state: BlessState) {
    for (source_path, mut rewrites) in bless_state.rewrites {
        let source_text = bless_state
            .sources
            .get(&source_path)
            .expect("bless source text should be captured");
        rewrites.sort_by_key(|rewrite| std::cmp::Reverse(rewrite.0.start));

        let mut blessed_text = source_text.clone();
        for (span, new_body) in &rewrites {
            blessed_text.replace_range(span.clone(), new_body);
        }

        fs::write(&source_path, &blessed_text).unwrap_or_else(|error| {
            panic!(
                "failed to write blessed fixture file `{}`: {error}",
                source_path.display()
            )
        });
        eprintln!(
            "blessed {} block(s) in {}",
            rewrites.len(),
            source_path.display()
        );
    }
}

pub fn read_fixture_suite(directory_path: &str) -> Vec<FixtureGroup> {
    let fixture_paths = collect_fixture_paths(directory_path);
    let mut groups = Vec::new();

    for fixture_path in fixture_paths {
        let fixture_text = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "failed to read fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        let fixture_file = parse_file(&fixture_text).unwrap_or_else(|error| {
            panic!(
                "failed to parse fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        groups.extend(fixture_file.groups);
    }

    groups
}

fn expected_outputs_for_fixture(fixture: &Fixture) -> Result<Vec<ExpectedFixtureOutput>, String> {
    match &fixture.kind {
        FixtureKind::Simple(case) => Ok(vec![ExpectedFixtureOutput {
            name: fixture.name.clone(),
            files: BTreeMap::from([(
                PathBuf::new(),
                ExpectedFileOutput::Exact(case.expected.clone()),
            )]),
        }]),
        FixtureKind::MultiFile(case) => expected_outputs_for_multi_file_fixture(case),
    }
}

pub fn expected_output_paths_for_multi_file_fixture(
    fixture: &MultiFileFixture,
) -> Result<Vec<FixtureExpectedPaths>, String> {
    expected_outputs_for_multi_file_fixture(fixture).map(|outputs| {
        outputs
            .into_iter()
            .map(|output| FixtureExpectedPaths {
                name: output.name,
                paths: output.files.into_keys().collect(),
            })
            .collect()
    })
}

pub fn expected_output_paths_for_generation(
    fixture: &MultiFileFixture,
    generation_index: usize,
) -> Result<FixtureExpectedPaths, String> {
    expected_output_paths_for_multi_file_fixture(fixture)?
        .into_iter()
        .nth(generation_index)
        .ok_or_else(|| {
            format!("missing expected output paths for generation index {generation_index}")
        })
}

fn expected_outputs_for_multi_file_fixture(
    fixture: &MultiFileFixture,
) -> Result<Vec<ExpectedFixtureOutput>, String> {
    let mut carried_outputs = BTreeMap::<PathBuf, ExpectedFileOutput>::new();
    let mut expected_outputs = Vec::with_capacity(fixture.generations.len() + 1);

    expected_outputs.push(ExpectedFixtureOutput {
        name: fixture.initial_generation.name.clone(),
        files: expected_outputs_for_generation_entries(
            &mut carried_outputs,
            &fixture.initial_generation.name,
            &fixture.initial_generation.entries,
        )?,
    });

    for generation in &fixture.generations {
        expected_outputs.push(ExpectedFixtureOutput {
            name: generation.name.clone(),
            files: expected_outputs_for_generation_entries(
                &mut carried_outputs,
                &generation.name,
                &generation.entries,
            )?,
        });
    }

    Ok(expected_outputs)
}

fn expected_outputs_for_generation_entries(
    carried_outputs: &mut BTreeMap<PathBuf, ExpectedFileOutput>,
    generation_name: &str,
    entries: &[crate::FixtureGenerationEntry],
) -> Result<BTreeMap<PathBuf, ExpectedFileOutput>, String> {
    let mut action_outputs = BTreeMap::new();

    for entry in entries {
        apply_generation_operation(carried_outputs, &entry.operation);

        if matches!(entry.operation, FixtureOperation::Action { .. }) {
            let expectation = entry.expectation.as_ref().ok_or_else(|| {
                format!("generation `{generation_name}` actions must have an output expectation")
            })?;
            action_outputs.insert(
                expectation.path.clone(),
                ExpectedFileOutput::from_fixture(&expectation.output),
            );
        } else if let Some(expectation) = &entry.expectation {
            carried_outputs.insert(
                expectation.path.clone(),
                ExpectedFileOutput::from_fixture(&expectation.output),
            );
        }

        let Some(path) = output_path_for_operation(&entry.operation) else {
            continue;
        };

        let has_expectation = if matches!(entry.operation, FixtureOperation::Action { .. }) {
            action_outputs.contains_key(path)
        } else {
            carried_outputs.contains_key(path)
        };

        if !has_expectation {
            return Err(format!(
                "generation `{generation_name}` uses `{}` without an explicit first expectation",
                path.display()
            ));
        }
    }

    let mut snapshot_outputs = carried_outputs.clone();
    snapshot_outputs.extend(action_outputs);
    Ok(snapshot_outputs)
}

fn apply_generation_operation(
    carried_outputs: &mut BTreeMap<PathBuf, ExpectedFileOutput>,
    operation: &FixtureOperation,
) {
    match operation {
        FixtureOperation::CreateDocument { .. }
        | FixtureOperation::EditDocument { .. }
        | FixtureOperation::Action { .. } => {}
        FixtureOperation::MoveDocument {
            source_path,
            destination_path,
        } => {
            if let Some(expected_output) = carried_outputs.remove(source_path) {
                carried_outputs.insert(destination_path.clone(), expected_output);
            }
        }
        FixtureOperation::DeleteDocument { path } => {
            carried_outputs.remove(path);
        }
    }
}

fn compare_fixture_outputs(
    fixture: &Fixture,
    fixture_name: &str,
    expected_outputs: &[ExpectedFixtureOutput],
    actual_outputs: &[Vec<FixtureRunFile>],
) -> Option<String> {
    if actual_outputs.len() != expected_outputs.len() {
        return Some(format!(
            "\u{1b}[1mfixture `{fixture_name}` failed\u{1b}[0m\n\u{1b}[1minput:\u{1b}[0m\n{}\n\u{1b}[1mexpected snapshots:\u{1b}[0m {}\n\u{1b}[1mactual snapshots:\u{1b}[0m {}",
            format!("{fixture:?}").trim_end(),
            expected_outputs.len(),
            actual_outputs.len()
        ));
    }

    for (expected_output, actual_output) in expected_outputs.iter().zip(actual_outputs) {
        let mut actual_files = BTreeMap::new();
        for file in actual_output {
            let replaced =
                actual_files.insert(file.path.clone(), file.output.trim_end().to_owned());
            if replaced.is_some() {
                return Some(format!(
                    "\u{1b}[1mfixture `{fixture_name}` failed\u{1b}[0m\n\u{1b}[1minput:\u{1b}[0m\n{}\n\u{1b}[1mactual:\u{1b}[0m duplicate output for `{}` in snapshot `{}`",
                    format!("{fixture:?}").trim_end(),
                    file.path.display(),
                    expected_output.name
                ));
            }
        }

        for path in actual_files.keys() {
            if !expected_output.files.contains_key(path) {
                return Some(render_output_mismatch(
                    fixture,
                    fixture_name,
                    expected_outputs,
                    actual_outputs,
                    &format!(
                        "unexpected output for `{}` in snapshot `{}`",
                        display_fixture_path(path),
                        expected_output.name
                    ),
                ));
            }
        }

        for (path, expected_file_output) in &expected_output.files {
            let Some(actual_output) = actual_files.get(path) else {
                return Some(render_output_mismatch(
                    fixture,
                    fixture_name,
                    expected_outputs,
                    actual_outputs,
                    &format!(
                        "missing output for `{}` in snapshot `{}`",
                        display_fixture_path(path),
                        expected_output.name
                    ),
                ));
            };

            if let ExpectedFileOutput::Exact(expected_output_text) = expected_file_output
                && actual_output != expected_output_text
            {
                return Some(render_output_mismatch(
                    fixture,
                    fixture_name,
                    expected_outputs,
                    actual_outputs,
                    "",
                ));
            }
        }
    }

    None
}

fn render_output_mismatch(
    fixture: &Fixture,
    fixture_name: &str,
    expected_outputs: &[ExpectedFixtureOutput],
    actual_outputs: &[Vec<FixtureRunFile>],
    message: &str,
) -> String {
    let mut rendered = format!(
        "\u{1b}[1mfixture `{fixture_name}` failed\u{1b}[0m\n\u{1b}[1minput:\u{1b}[0m\n{}",
        format!("{fixture:?}").trim_end(),
    );

    if !message.is_empty() {
        rendered.push('\n');
        rendered.push_str("\u{1b}[1mreason:\u{1b}[0m ");
        rendered.push_str(message);
    }

    rendered.push_str("\n\u{1b}[1mexpected:\u{1b}[0m\n");
    rendered.push_str(&render_expected_outputs(expected_outputs));
    rendered.push_str("\n\u{1b}[1mactual:\u{1b}[0m\n");
    rendered.push_str(&render_actual_outputs(expected_outputs, actual_outputs));
    rendered
}

fn render_expected_outputs(outputs: &[ExpectedFixtureOutput]) -> String {
    outputs
        .iter()
        .map(render_expected_snapshot)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_expected_snapshot(output: &ExpectedFixtureOutput) -> String {
    let rendered_files = render_expected_files(&output.files);
    if output.name.is_empty() {
        return rendered_files;
    }

    format!("## {}\n{}", output.name, rendered_files)
}

fn render_expected_files(files: &BTreeMap<PathBuf, ExpectedFileOutput>) -> String {
    if files.len() == 1 {
        let mut iter = files.iter();
        let Some((path, expected_output)) = iter.next() else {
            return String::new();
        };
        if path.as_os_str().is_empty() {
            return render_expected_file_output(expected_output);
        }
    }

    files
        .iter()
        .map(|(path, expected_output)| {
            format!(
                "== {} ==\n{}",
                display_fixture_path(path),
                render_expected_file_output(expected_output)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_expected_file_output(expected_output: &ExpectedFileOutput) -> String {
    match expected_output {
        ExpectedFileOutput::Exact(output) => output.clone(),
        ExpectedFileOutput::Any => "any".to_owned(),
    }
}

fn render_actual_outputs(
    expected_outputs: &[ExpectedFixtureOutput],
    actual_outputs: &[Vec<FixtureRunFile>],
) -> String {
    expected_outputs
        .iter()
        .zip(actual_outputs)
        .map(|(expected_output, actual_output)| {
            render_actual_snapshot(&expected_output.name, actual_output)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_actual_snapshot(snapshot_name: &str, output: &[FixtureRunFile]) -> String {
    let rendered_files = render_actual_files(output);
    if snapshot_name.is_empty() {
        return rendered_files;
    }

    format!("## {}\n{}", snapshot_name, rendered_files)
}

fn render_actual_files(files: &[FixtureRunFile]) -> String {
    if files.len() == 1 && files[0].path.as_os_str().is_empty() {
        return files[0].output.trim_end().to_owned();
    }

    files
        .iter()
        .map(|file| {
            format!(
                "== {} ==\n{}",
                display_fixture_path(&file.path),
                file.output.trim_end()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn collect_fixture_paths(directory_path: &str) -> Vec<PathBuf> {
    let mut fixture_paths = Vec::new();
    collect_fixture_paths_recursively(Path::new(directory_path), &mut fixture_paths);
    fixture_paths.sort();
    fixture_paths
}

fn collect_fixture_paths_recursively(directory_path: &Path, fixture_paths: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory_path).unwrap_or_else(|error| {
        panic!(
            "failed to read fixture directory `{}`: {error}",
            directory_path.display()
        )
    });

    let mut entry_paths = entries
        .map(|entry| {
            entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read entry in fixture directory `{}`: {error}",
                    directory_path.display()
                )
            })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entry_paths.sort();

    for entry_path in entry_paths {
        if entry_path.is_dir() {
            collect_fixture_paths_recursively(&entry_path, fixture_paths);
            continue;
        }

        if entry_path
            .extension()
            .is_some_and(|extension| extension == "test")
        {
            fixture_paths.push(entry_path);
        }
    }
}

fn output_path_for_operation(operation: &FixtureOperation) -> Option<&PathBuf> {
    match operation {
        FixtureOperation::CreateDocument { path, .. } => Some(path),
        FixtureOperation::EditDocument { path, .. } => Some(path),
        FixtureOperation::MoveDocument {
            destination_path, ..
        } => Some(destination_path),
        FixtureOperation::DeleteDocument { .. } => None,
        FixtureOperation::Action { path, .. } => Some(path),
    }
}

fn display_fixture_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "<main>".to_owned()
    } else {
        path.display().to_string()
    }
}

struct BlessState {
    targets: BTreeMap<String, BlessTarget>,
    sources: BTreeMap<PathBuf, String>,
    rewrites: BTreeMap<PathBuf, Vec<(Range<usize>, String)>>,
}

struct BlessTarget {
    source_path: PathBuf,
    spans: Vec<Range<usize>>,
}

struct ExpectationLocation {
    generation_index: usize,
    path: PathBuf,
    expected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedFixtureOutput {
    name: String,
    files: BTreeMap<PathBuf, ExpectedFileOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpectedFileOutput {
    Exact(String),
    Any,
}

impl ExpectedFileOutput {
    fn from_fixture(expected_output: &FixtureExpectedOutput) -> Self {
        match expected_output {
            FixtureExpectedOutput::Exact(output) => Self::Exact(output.clone()),
            FixtureExpectedOutput::Any => Self::Any,
        }
    }
}
