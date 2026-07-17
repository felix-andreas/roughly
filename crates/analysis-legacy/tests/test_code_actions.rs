use {
    analysis::{
        Analysis, CheckConfig, LintConfig, ide,
        text::{TextPosition, TextRange},
    },
    std::path::PathBuf,
};

fn whole_file() -> TextRange {
    TextRange {
        start: TextPosition {
            line_index: 0,
            character_index: 0,
        },
        end: TextPosition {
            line_index: 1000,
            character_index: 0,
        },
    }
}

#[test]
fn if_unknown_annotation_offers_the_trust_rewrite() {
    let mut analysis = Analysis::new(
        PathBuf::from("/project"),
        LintConfig::default(),
        CheckConfig::default(),
    );
    let path = PathBuf::from("/project/R/main.R");
    analysis
        .add_document_from_source(path.clone(), "#: @if-unknown integer\nvalue <- foreign()\n")
        .expect("document syncs");

    let actions = ide::code_actions(&mut analysis, &path, whole_file());
    let rewrite = actions
        .iter()
        .find(|action| action.title.contains("@trust"))
        .unwrap_or_else(|| panic!("expected the @trust rewrite, got: {actions:?}"));

    let edits = rewrite.edits.get(&path).expect("edit targets the file");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].replacement_text, "@trust");
    assert_eq!(edits[0].range.start.line_index, 0);
    assert_eq!(edits[0].range.start.character_index, 3);
    assert_eq!(edits[0].range.end.character_index, 3 + "@if-unknown".len());
}

// The inserted annotation must round-trip: a numeric-generic binding renders with its constraint
// (`<T: numeric>`, not a bare `<T>` whose rigid variable the body's arithmetic could not
// constrain), and re-checking the source with the inserted line produces no diagnostics.
#[test]
fn inferred_annotation_insert_keeps_the_numeric_constraint_and_round_trips() {
    let mut analysis = Analysis::new(
        PathBuf::from("/project"),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
            strict: false,
        },
    );
    let path = PathBuf::from("/project/R/main.R");
    let source = "add_one <- function(x) x + 1L\n";
    analysis
        .add_document_from_source(path.clone(), source)
        .expect("document syncs");

    let actions = ide::code_actions(&mut analysis, &path, whole_file());
    let insert = actions
        .iter()
        .find(|action| action.title.contains("`add_one`"))
        .unwrap_or_else(|| panic!("expected the annotation insert, got: {actions:?}"));
    let edits = insert.edits.get(&path).expect("edit targets the file");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].replacement_text, "#: <T: numeric> fn(x: T) -> T\n");

    let annotated_source = format!("{}{}", edits[0].replacement_text, source);
    let mut recheck = Analysis::new(
        PathBuf::from("/project"),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
            strict: false,
        },
    );
    recheck
        .add_document_from_source(path.clone(), &annotated_source)
        .expect("document syncs");
    analysis::run_full(&mut recheck);
    let document_id = recheck
        .document_id_for_path(&path)
        .expect("document is tracked");
    let diagnostics = recheck.document_diagnostics(document_id);
    assert!(
        diagnostics.is_empty(),
        "inserted annotation must check cleanly, got: {diagnostics:?}"
    );
}
