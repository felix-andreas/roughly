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
