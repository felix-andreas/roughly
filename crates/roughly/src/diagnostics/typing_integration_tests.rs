use crate::diagnostics;
use crate::tree;
use ropey::Rope;

#[test]
fn test_typing_integration() {
    let source = r#"
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
"#;

    let mut parser = tree::new_parser();
    let tree = tree::parse(&mut parser, source, None);
    let rope = Rope::from_str(source);
    
    let config = diagnostics::Config {
        case: crate::config::Case::Snake,
        experimental: true,
    };
    
    let diagnostics = diagnostics::analyze(tree.root_node(), &rope, config, true);
    
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
    let source = r#"
# Valid code with type annotations
age <- 25 #: numeric
name <- "Alice" #: character
is_adult <- TRUE #: logical

#: @param x numeric
#: @return numeric
double_value <- function(x) {
  x * 2
}
"#;

    let mut parser = tree::new_parser();
    let tree = tree::parse(&mut parser, source, None);
    let rope = Rope::from_str(source);
    
    let config = diagnostics::Config {
        case: crate::config::Case::Snake,
        experimental: true,
    };
    
    let diagnostics = diagnostics::analyze(tree.root_node(), &rope, config, true);
    
    // Should have no type error diagnostics
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("typing"))
        .collect();
    
    assert!(type_errors.is_empty(), "Expected no type checking errors for valid code");
}

#[test]
fn test_integer_numeric_coercion() {
    let source = r#"
# Integer should be assignable to numeric
count <- 42L #: numeric
pi_value <- 3.14 #: numeric
"#;

    let mut parser = tree::new_parser();
    let tree = tree::parse(&mut parser, source, None);
    let rope = Rope::from_str(source);
    
    let config = diagnostics::Config {
        case: crate::config::Case::Snake,
        experimental: true,
    };
    
    let diagnostics = diagnostics::analyze(tree.root_node(), &rope, config, true);
    
    // Should have no type error diagnostics for integer to numeric coercion
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("typing"))
        .collect();
    
    assert!(type_errors.is_empty(), "Expected no type errors for integer to numeric coercion");
}