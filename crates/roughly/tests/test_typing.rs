use {
    indoc::indoc,
    ropey::Rope,
    roughly::{
        config::Case,
        diagnostics::{self, Config},
        lsp_types::Diagnostic,
        tree,
    },
};

fn setup(source: &str) -> Vec<Diagnostic> {
    let mut parser = tree::new_parser();
    let tree = tree::parse(&mut parser, source, None);
    let rope = Rope::from_str(source);

    let config = Config {
        case: Case::Snake,
        experimental_unused: false,
        experimental_typing: true,
    };

    diagnostics::analyze(tree.root_node(), &rope, config, true)
}

#[test]
fn test_typing_integration() {
    let source = indoc! { r#"
        # Valid type annotation
        x <- 42 #: numeric

        # Invalid type annotation - should produce error
        y <- "hello" #: numeric

        # Parameter annotation
        #: @param count numeric
        #: @return logical
        is_positive <- function(count) {
          count > 0
        }
    "# };

    let diagnostics = setup(source);

    // Should have at least one type error diagnostic
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("typing"))
        .collect();

    assert!(!type_errors.is_empty(), "Expected type checking errors");

    // Check that the error message contains expected text
    let error_msg = &type_errors[0].message;
    assert!(error_msg.contains("Type mismatch"));
    assert!(error_msg.contains("numeric"));
    assert!(error_msg.contains("character"));
}

#[test]
fn test_valid_code_no_type_errors() {
    let source = indoc! { r#"
        # Valid code with type annotations
        age <- 25 #: numeric
        name <- "Alice" #: character
        is_adult <- TRUE #: logical

        #: @param x numeric
        #: @return numeric
        double_value <- function(x) {
          x * 2
        }
    "# };

    let diagnostics = setup(source);

    // Should have no type error diagnostics
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("typing"))
        .collect();

    assert!(
        type_errors.is_empty(),
        "Expected no type checking errors for valid code"
    );
}

#[test]
fn test_integer_numeric_coercion() {
    let source = indoc! { r#"
        # Integer should be assignable to numeric
        count <- 42L #: numeric
        pi_value <- 3.14 #: numeric
    "# };

    let diagnostics = setup(source);

    // Should have no type error diagnostics for integer to numeric coercion
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("typing"))
        .collect();

    assert!(
        type_errors.is_empty(),
        "Expected no type errors for integer to numeric coercion"
    );
}
