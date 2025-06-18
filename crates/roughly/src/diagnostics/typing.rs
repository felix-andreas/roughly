use {
    crate::{
        diagnostics::error,
        lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range},
    },
    ropey::Rope,
    thiserror::Error,
    tree_sitter::Node,
    typing::{TypeCheckError, TypeChecker},
};

#[derive(Error, Debug)]
pub enum TypeDiagnosticsError {
    #[error("Type checking error: {0}")]
    TypeCheckError(#[from] TypeCheckError),
}

/// Analyze a source file for type errors
pub fn analyze(node: Node, rope: &Rope) -> Result<Vec<Diagnostic>, TypeDiagnosticsError> {
    let mut diagnostics = Vec::new();
    let source = rope.to_string();

    let mut type_checker = TypeChecker::new();

    // First pass: extract type annotations
    match type_checker.extract_type_annotations(&source) {
        Ok(_annotations) => {
            // For MVP, we'll do basic type checking on annotated variables
            // More sophisticated analysis can be added later

            // Check for simple type violations in the source
            for (line_num, line) in source.lines().enumerate() {
                if let Some(diagnostic) =
                    check_line_for_type_errors(&mut type_checker, line, line_num)
                {
                    diagnostics.push(diagnostic);
                }
            }
        }
        Err(type_error) => {
            // Report parse errors as diagnostics
            let diagnostic = error(node, format!("Type annotation error: {}", type_error));
            diagnostics.push(diagnostic);
        }
    }

    Ok(diagnostics)
}

/// Check a single line for basic type errors
fn check_line_for_type_errors(
    type_checker: &mut TypeChecker,
    line: &str,
    line_num: usize,
) -> Option<Diagnostic> {
    // Look for assignment patterns with type annotations
    if line.contains("#:") && (line.contains("<-") || line.contains("=")) {
        if let Some(var_name) = extract_variable_name(line) {
            if let Some(assigned_value) = extract_assigned_value(line) {
                let inferred_type = type_checker.infer_expression_type(assigned_value);

                // Check if the assignment is compatible with the declared type
                if let Err(type_error) = type_checker.check_assignment(&var_name, &inferred_type) {
                    return Some(create_type_error_diagnostic(
                        line_num,
                        line,
                        format!(
                            "Type mismatch in assignment to '{}': {}",
                            var_name, type_error
                        ),
                    ));
                }
            }
        }
    }

    None
}

/// Extract variable name from assignment
fn extract_variable_name(line: &str) -> Option<String> {
    if let Some(pos) = line.find("<-") {
        let var_part = line[..pos].trim();
        if is_valid_identifier(var_part) {
            return Some(var_part.to_string());
        }
    } else if let Some(pos) = line.find("=") {
        // Make sure it's not part of a comment or comparison
        if !line[..pos].contains("#") && !line[pos..].starts_with("==") {
            let var_part = line[..pos].trim();
            if is_valid_identifier(var_part) {
                return Some(var_part.to_string());
            }
        }
    }
    None
}

/// Extract the assigned value from assignment
fn extract_assigned_value(line: &str) -> Option<&str> {
    if let Some(pos) = line.find("<-") {
        let value_part = &line[pos + 2..];
        if let Some(comment_pos) = value_part.find("#:") {
            return Some(value_part[..comment_pos].trim());
        }
    } else if let Some(pos) = line.find("=") {
        if !line[pos..].starts_with("==") {
            let value_part = &line[pos + 1..];
            if let Some(comment_pos) = value_part.find("#:") {
                return Some(value_part[..comment_pos].trim());
            }
        }
    }
    None
}

/// Check if a string is a valid R identifier
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let mut chars = s.chars();
    let first_char = chars.next().unwrap();

    // First character must be letter or dot
    if !first_char.is_alphabetic() && first_char != '.' {
        return false;
    }

    // If starts with dot, second character must not be digit
    if first_char == '.' {
        if let Some(second_char) = chars.next() {
            if second_char.is_ascii_digit() {
                return false;
            }
        }
    }

    // Rest can be alphanumeric, underscore, or dot
    for ch in chars {
        if !ch.is_alphanumeric() && ch != '_' && ch != '.' {
            return false;
        }
    }

    true
}

/// Create a diagnostic for a type error
fn create_type_error_diagnostic(line_num: usize, line: &str, message: String) -> Diagnostic {
    Diagnostic {
        message,
        severity: Some(DiagnosticSeverity::ERROR),
        range: Range::new(
            Position::new(line_num as u32, 0),
            Position::new(line_num as u32, line.len() as u32),
        ),
        code: None,
        code_description: None,
        source: Some("typing".to_string()),
        related_information: None,
        tags: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifier() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("my_var"));
        assert!(is_valid_identifier("var123"));
        assert!(is_valid_identifier(".hidden"));
        assert!(is_valid_identifier("data.frame"));

        assert!(!is_valid_identifier("123var"));
        assert!(!is_valid_identifier(".123"));
        assert!(!is_valid_identifier("var-name"));
        assert!(!is_valid_identifier(""));
    }

    #[test]
    fn test_extract_variable_name() {
        assert_eq!(extract_variable_name("x <- 4"), Some("x".to_string()));
        assert_eq!(
            extract_variable_name("my_var <- 'hello'"),
            Some("my_var".to_string())
        );
        assert_eq!(
            extract_variable_name("result = compute()"),
            Some("result".to_string())
        );
        assert_eq!(extract_variable_name("x == 4"), None); // comparison, not assignment
        assert_eq!(extract_variable_name("invalid syntax"), None);
    }

    #[test]
    fn test_extract_assigned_value() {
        assert_eq!(extract_assigned_value("x <- 4 #: numeric"), Some("4"));
        assert_eq!(
            extract_assigned_value("name <- 'hello' #: character"),
            Some("'hello'")
        );
        assert_eq!(extract_assigned_value("x = 3.14 #: numeric"), Some("3.14"));
    }
}
