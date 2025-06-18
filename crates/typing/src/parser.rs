use crate::types::RType;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Invalid type syntax: {message}")]
    InvalidSyntax { message: String },
    #[error("Unexpected token: {token}")]
    UnexpectedToken { token: String },
    #[error("Unterminated type expression")]
    UnterminatedExpression,
}

/// Parser for type annotations in JSDoc-style comments
pub struct TypeParser {
    input: String,
    position: usize,
}

impl TypeParser {
    pub fn new(input: String) -> Self {
        Self { input, position: 0 }
    }

    /// Parse a type annotation from a string
    pub fn parse_type(&mut self) -> Result<RType, ParseError> {
        self.skip_whitespace();
        self.parse_primary_type()
    }

    /// Parse a function parameter annotation like "@param name type"
    pub fn parse_param_annotation(line: &str) -> Result<(String, RType), ParseError> {
        let trimmed = line.trim();
        if !trimmed.starts_with("@param") {
            return Err(ParseError::InvalidSyntax {
                message: "Expected @param annotation".to_string(),
            });
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(ParseError::InvalidSyntax {
                message: "Expected @param name type".to_string(),
            });
        }

        let param_name = parts[1].to_string();
        let type_str = parts[2..].join(" ");
        
        let mut parser = TypeParser::new(type_str);
        let r_type = parser.parse_type()?;
        
        Ok((param_name, r_type))
    }

    /// Parse a return type annotation like "@return type"
    pub fn parse_return_annotation(line: &str) -> Result<RType, ParseError> {
        let trimmed = line.trim();
        if !trimmed.starts_with("@return") {
            return Err(ParseError::InvalidSyntax {
                message: "Expected @return annotation".to_string(),
            });
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(ParseError::InvalidSyntax {
                message: "Expected @return type".to_string(),
            });
        }

        let type_str = parts[1..].join(" ");
        let mut parser = TypeParser::new(type_str);
        parser.parse_type()
    }

    /// Parse a variable type annotation like "variable #: type"
    pub fn parse_variable_annotation(line: &str) -> Result<RType, ParseError> {
        if let Some(pos) = line.find("#:") {
            let type_part = &line[pos + 2..].trim();
            let mut parser = TypeParser::new(type_part.to_string());
            parser.parse_type()
        } else {
            Err(ParseError::InvalidSyntax {
                message: "No type annotation found".to_string(),
            })
        }
    }

    fn parse_primary_type(&mut self) -> Result<RType, ParseError> {
        self.skip_whitespace();
        
        let _start_pos = self.position;
        
        // Parse identifier
        let type_name = self.parse_identifier()?;
        
        match type_name.as_str() {
            "null" => Ok(RType::Null),
            "numeric" => self.parse_array_suffix(RType::Numeric, RType::NumericArray),
            "character" => self.parse_array_suffix(RType::Character, RType::CharacterArray),
            "logical" => self.parse_array_suffix(RType::Logical, RType::LogicalArray),
            "integer" => self.parse_array_suffix(RType::Integer, RType::IntegerArray),
            "any" => Ok(RType::Any),
            "unknown" => Ok(RType::Unknown),
            "list" => self.parse_list_type(),
            "fn" => self.parse_function_type(),
            _ => Err(ParseError::InvalidSyntax {
                message: format!("Unknown type: {}", type_name),
            }),
        }
    }

    fn parse_array_suffix(&mut self, scalar_type: RType, array_type: RType) -> Result<RType, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some('[') && self.peek_at(self.position + 1) == Some(']') {
            self.position += 2;
            Ok(array_type)
        } else {
            Ok(scalar_type)
        }
    }

    fn parse_list_type(&mut self) -> Result<RType, ParseError> {
        self.skip_whitespace();
        
        match self.peek() {
            Some('[') => {
                // Homogeneous list: list[type]
                self.advance(); // consume '['
                let inner_type = self.parse_primary_type()?;
                self.skip_whitespace();
                if self.peek() != Some(']') {
                    return Err(ParseError::UnterminatedExpression);
                }
                self.advance(); // consume ']'
                Ok(RType::List(Box::new(inner_type)))
            }
            Some('{') => {
                // Record: list{name: type, ...}
                self.advance(); // consume '{'
                let mut fields = HashMap::new();
                
                loop {
                    self.skip_whitespace();
                    if self.peek() == Some('}') {
                        self.advance();
                        break;
                    }
                    
                    let field_name = self.parse_identifier()?;
                    self.skip_whitespace();
                    
                    if self.peek() != Some(':') {
                        return Err(ParseError::UnexpectedToken {
                            token: format!("{:?}", self.peek()),
                        });
                    }
                    self.advance(); // consume ':'
                    
                    let field_type = self.parse_primary_type()?;
                    fields.insert(field_name, field_type);
                    
                    self.skip_whitespace();
                    if self.peek() == Some(',') {
                        self.advance(); // consume ','
                    } else if self.peek() != Some('}') {
                        return Err(ParseError::UnexpectedToken {
                            token: format!("{:?}", self.peek()),
                        });
                    }
                }
                
                Ok(RType::Record(fields))
            }
            Some('(') => {
                // Tuple: list(type1, type2, ...)
                self.advance(); // consume '('
                let mut types = Vec::new();
                
                loop {
                    self.skip_whitespace();
                    if self.peek() == Some(')') {
                        self.advance();
                        break;
                    }
                    
                    let field_type = self.parse_primary_type()?;
                    types.push(field_type);
                    
                    self.skip_whitespace();
                    if self.peek() == Some(',') {
                        self.advance(); // consume ','
                    } else if self.peek() != Some(')') {
                        return Err(ParseError::UnexpectedToken {
                            token: format!("{:?}", self.peek()),
                        });
                    }
                }
                
                Ok(RType::Tuple(types))
            }
            _ => {
                // Just "list" without specification - assume list[any]
                Ok(RType::List(Box::new(RType::Any)))
            }
        }
    }

    fn parse_function_type(&mut self) -> Result<RType, ParseError> {
        self.skip_whitespace();
        if self.peek() != Some('(') {
            return Err(ParseError::UnexpectedToken {
                token: format!("{:?}", self.peek()),
            });
        }
        
        self.advance(); // consume '('
        let mut params = Vec::new();
        let mut named_params = HashMap::new();
        
        loop {
            self.skip_whitespace();
            if self.peek() == Some(')') {
                self.advance();
                break;
            }
            
            // Try to parse as named parameter (name: type)
            let start_pos = self.position;
            if let Ok(name) = self.parse_identifier() {
                self.skip_whitespace();
                if self.peek() == Some(':') {
                    self.advance(); // consume ':'
                    let param_type = self.parse_primary_type()?;
                    named_params.insert(name, param_type);
                } else {
                    // Not a named parameter, backtrack and parse as positional
                    self.position = start_pos;
                    let param_type = self.parse_primary_type()?;
                    params.push(param_type);
                }
            } else {
                let param_type = self.parse_primary_type()?;
                params.push(param_type);
            }
            
            self.skip_whitespace();
            if self.peek() == Some(',') {
                self.advance(); // consume ','
            } else if self.peek() != Some(')') {
                return Err(ParseError::UnexpectedToken {
                    token: format!("{:?}", self.peek()),
                });
            }
        }
        
        // Parse return type if present
        self.skip_whitespace();
        let return_type = if self.peek() == Some('-') && self.peek_at(self.position + 1) == Some('>') {
            self.position += 2; // consume '->'
            Box::new(self.parse_primary_type()?)
        } else {
            Box::new(RType::Any) // Default return type
        };
        
        Ok(RType::Function {
            params,
            named_params,
            return_type,
        })
    }

    fn parse_identifier(&mut self) -> Result<String, ParseError> {
        self.skip_whitespace();
        let start = self.position;
        
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        
        if start == self.position {
            return Err(ParseError::InvalidSyntax {
                message: "Expected identifier".to_string(),
            });
        }
        
        Ok(self.input[start..self.position].to_string())
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.chars().nth(self.position)
    }

    fn peek_at(&self, position: usize) -> Option<char> {
        self.input.chars().nth(position)
    }

    fn advance(&mut self) {
        self.position += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_types() {
        let mut parser = TypeParser::new("numeric".to_string());
        assert_eq!(parser.parse_type().unwrap(), RType::Numeric);

        let mut parser = TypeParser::new("character[]".to_string());
        assert_eq!(parser.parse_type().unwrap(), RType::CharacterArray);

        let mut parser = TypeParser::new("null".to_string());
        assert_eq!(parser.parse_type().unwrap(), RType::Null);
    }

    #[test]
    fn test_parse_list_types() {
        let mut parser = TypeParser::new("list[numeric]".to_string());
        assert_eq!(
            parser.parse_type().unwrap(),
            RType::List(Box::new(RType::Numeric))
        );

        let mut parser = TypeParser::new("list(character, numeric)".to_string());
        assert_eq!(
            parser.parse_type().unwrap(),
            RType::Tuple(vec![RType::Character, RType::Numeric])
        );
    }

    #[test]
    fn test_parse_param_annotation() {
        let result = TypeParser::parse_param_annotation("@param x numeric").unwrap();
        assert_eq!(result, ("x".to_string(), RType::Numeric));

        let result = TypeParser::parse_param_annotation("@param items character[]").unwrap();
        assert_eq!(result, ("items".to_string(), RType::CharacterArray));
    }

    #[test]
    fn test_parse_return_annotation() {
        let result = TypeParser::parse_return_annotation("@return logical").unwrap();
        assert_eq!(result, RType::Logical);
    }

    #[test]
    fn test_parse_variable_annotation() {
        let result = TypeParser::parse_variable_annotation("x <- 4 #: numeric").unwrap();
        assert_eq!(result, RType::Numeric);

        let result = TypeParser::parse_variable_annotation("items <- c() #: character[]").unwrap();
        assert_eq!(result, RType::CharacterArray);
    }
}