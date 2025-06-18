use crate::types::{RType, TypeAnnotation, SourceRange};
use crate::parser::{TypeParser, ParseError};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("Type mismatch: expected {expected}, found {actual}")]
    TypeMismatch { expected: RType, actual: RType },
    #[error("Undefined variable: {name}")]
    UndefinedVariable { name: String },
    #[error("Parse error: {error}")]
    ParseError { error: ParseError },
    #[error("Invalid function call: {message}")]
    InvalidFunctionCall { message: String },
}

/// Symbol information with type
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub r_type: RType,
    pub source_range: Option<SourceRange>,
}

/// Type checking context
#[derive(Debug, Clone)]
pub struct TypeContext {
    /// Symbol table mapping variable names to their types
    symbols: HashMap<String, Symbol>,
    /// Function definitions
    functions: HashMap<String, RType>,
}

impl TypeContext {
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub fn define_symbol(&mut self, name: String, symbol: Symbol) {
        self.symbols.insert(name, symbol);
    }

    pub fn get_symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }

    pub fn define_function(&mut self, name: String, function_type: RType) {
        self.functions.insert(name, function_type);
    }

    pub fn get_function(&self, name: &str) -> Option<&RType> {
        self.functions.get(name)
    }
}

/// Main type checker
pub struct TypeChecker {
    context: TypeContext,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            context: TypeContext::new(),
        }
    }

    /// Extract type annotations from R source code
    pub fn extract_type_annotations(&mut self, source: &str) -> Result<Vec<TypeAnnotation>, TypeCheckError> {
        let mut annotations = Vec::new();
        
        for (line_num, line) in source.lines().enumerate() {
            // Look for type annotations in comments
            if let Some(comment_start) = line.find("#:") {
                let comment_part = &line[comment_start + 2..].trim();
                
                // Check if it's a parameter or return annotation
                if comment_part.starts_with("@param") {
                    match TypeParser::parse_param_annotation(comment_part) {
                        Ok((param_name, r_type)) => {
                            let symbol = Symbol {
                                name: param_name.clone(),
                                r_type: r_type.clone(),
                                source_range: Some(SourceRange {
                                    start_line: line_num,
                                    start_col: comment_start,
                                    end_line: line_num,
                                    end_col: line.len(),
                                }),
                            };
                            self.context.define_symbol(param_name, symbol);
                            
                            annotations.push(TypeAnnotation {
                                r_type,
                                source_range: Some(SourceRange {
                                    start_line: line_num,
                                    start_col: comment_start,
                                    end_line: line_num,
                                    end_col: line.len(),
                                }),
                            });
                        }
                        Err(e) => return Err(TypeCheckError::ParseError { error: e }),
                    }
                } else if comment_part.starts_with("@return") {
                    match TypeParser::parse_return_annotation(comment_part) {
                        Ok(r_type) => {
                            annotations.push(TypeAnnotation {
                                r_type,
                                source_range: Some(SourceRange {
                                    start_line: line_num,
                                    start_col: comment_start,
                                    end_line: line_num,
                                    end_col: line.len(),
                                }),
                            });
                        }
                        Err(e) => return Err(TypeCheckError::ParseError { error: e }),
                    }
                } else {
                    // Variable type annotation
                    match TypeParser::parse_variable_annotation(line) {
                        Ok(r_type) => {
                            // Try to extract variable name from the line
                            if let Some(var_name) = self.extract_variable_name(line) {
                                let symbol = Symbol {
                                    name: var_name.clone(),
                                    r_type: r_type.clone(),
                                    source_range: Some(SourceRange {
                                        start_line: line_num,
                                        start_col: 0,
                                        end_line: line_num,
                                        end_col: comment_start,
                                    }),
                                };
                                self.context.define_symbol(var_name, symbol);
                            }
                            
                            annotations.push(TypeAnnotation {
                                r_type,
                                source_range: Some(SourceRange {
                                    start_line: line_num,
                                    start_col: comment_start,
                                    end_line: line_num,
                                    end_col: line.len(),
                                }),
                            });
                        }
                        Err(e) => return Err(TypeCheckError::ParseError { error: e }),
                    }
                }
            }
        }
        
        Ok(annotations)
    }

    /// Extract variable name from assignment line
    fn extract_variable_name(&self, line: &str) -> Option<String> {
        // Look for patterns like "var <-" or "var ="
        if let Some(pos) = line.find("<-") {
            let var_part = line[..pos].trim();
            if var_part.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                return Some(var_part.to_string());
            }
        } else if let Some(pos) = line.find("=") {
            let var_part = line[..pos].trim();
            if var_part.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                return Some(var_part.to_string());
            }
        }
        None
    }

    /// Check if an assignment is type-safe
    pub fn check_assignment(&self, var_name: &str, assigned_type: &RType) -> Result<(), TypeCheckError> {
        if let Some(symbol) = self.context.get_symbol(var_name) {
            if !assigned_type.is_compatible_with(&symbol.r_type) {
                return Err(TypeCheckError::TypeMismatch {
                    expected: symbol.r_type.clone(),
                    actual: assigned_type.clone(),
                });
            }
        }
        Ok(())
    }

    /// Infer type from R literal values
    pub fn infer_literal_type(&self, value: &str) -> RType {
        let trimmed = value.trim();
        
        // NULL
        if trimmed.eq_ignore_ascii_case("null") {
            return RType::Null;
        }
        
        // Logical values
        if trimmed.eq_ignore_ascii_case("true") || trimmed.eq_ignore_ascii_case("false") {
            return RType::Logical;
        }
        
        // Numeric values
        if trimmed.parse::<f64>().is_ok() {
            if trimmed.contains('.') {
                return RType::Numeric;
            } else {
                return RType::Integer;
            }
        }
        
        // Character strings
        if (trimmed.starts_with('"') && trimmed.ends_with('"')) ||
           (trimmed.starts_with('\'') && trimmed.ends_with('\'')) {
            return RType::Character;
        }
        
        // Default to unknown for complex expressions
        RType::Unknown
    }

    /// Get type context for inspection
    pub fn get_context(&self) -> &TypeContext {
        &self.context
    }

    /// Basic type checking for simple expressions
    pub fn check_expression(&self, expression: &str) -> Result<RType, TypeCheckError> {
        let trimmed = expression.trim();
        
        // Check if it's a variable reference
        if let Some(symbol) = self.context.get_symbol(trimmed) {
            return Ok(symbol.r_type.clone());
        }
        
        // Check if it's a literal
        let literal_type = self.infer_literal_type(trimmed);
        if !matches!(literal_type, RType::Unknown) {
            return Ok(literal_type);
        }
        
        // For complex expressions, return Unknown for now
        Ok(RType::Unknown)
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_annotation_extraction() {
        let mut checker = TypeChecker::new();
        let source = r#"
x <- 4 #: numeric
y <- "hello" #: character
#: @param a numeric
#: @return logical
        "#;
        
        let annotations = checker.extract_type_annotations(source).unwrap();
        assert_eq!(annotations.len(), 4);
        
        // Check that symbols were defined
        assert!(checker.context.get_symbol("x").is_some());
        assert!(checker.context.get_symbol("y").is_some());
        assert!(checker.context.get_symbol("a").is_some());
    }

    #[test]
    fn test_literal_type_inference() {
        let checker = TypeChecker::new();
        
        assert_eq!(checker.infer_literal_type("42"), RType::Integer);
        assert_eq!(checker.infer_literal_type("3.14"), RType::Numeric);
        assert_eq!(checker.infer_literal_type("\"hello\""), RType::Character);
        assert_eq!(checker.infer_literal_type("TRUE"), RType::Logical);
        assert_eq!(checker.infer_literal_type("NULL"), RType::Null);
    }

    #[test]
    fn test_assignment_checking() {
        let mut checker = TypeChecker::new();
        
        // Define a symbol
        let symbol = Symbol {
            name: "x".to_string(),
            r_type: RType::Numeric,
            source_range: None,
        };
        checker.context.define_symbol("x".to_string(), symbol);
        
        // Valid assignment
        assert!(checker.check_assignment("x", &RType::Numeric).is_ok());
        assert!(checker.check_assignment("x", &RType::Integer).is_ok()); // Integer can be assigned to numeric
        
        // Invalid assignment
        assert!(checker.check_assignment("x", &RType::Character).is_err());
    }

    #[test]
    fn test_variable_name_extraction() {
        let checker = TypeChecker::new();
        
        assert_eq!(checker.extract_variable_name("x <- 4"), Some("x".to_string()));
        assert_eq!(checker.extract_variable_name("my_var <- 'hello'"), Some("my_var".to_string()));
        assert_eq!(checker.extract_variable_name("result = compute()"), Some("result".to_string()));
        assert_eq!(checker.extract_variable_name("invalid syntax"), None);
    }
}