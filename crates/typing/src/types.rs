use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents the basic types in R's type system
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RType {
    /// null type
    Null,
    /// Scalar numeric (length 1)
    Numeric,
    /// Scalar character (length 1) 
    Character,
    /// Scalar logical (length 1)
    Logical,
    /// Scalar integer (length 1)
    Integer,
    /// Numeric vector (arbitrary length)
    NumericArray,
    /// Character vector (arbitrary length)
    CharacterArray,
    /// Logical vector (arbitrary length)
    LogicalArray,
    /// Integer vector (arbitrary length)
    IntegerArray,
    /// Homogeneous list
    List(Box<RType>),
    /// Heterogeneous record with named fields
    Record(HashMap<String, RType>),
    /// Heterogeneous tuple with positional fields
    Tuple(Vec<RType>),
    /// Function type
    Function {
        params: Vec<RType>,
        named_params: HashMap<String, RType>,
        return_type: Box<RType>,
    },
    /// Type could not be inferred
    Unknown,
    /// Explicit opt-out of type checking
    Any,
}

impl RType {
    /// Check if this type is a scalar (length 1)
    pub fn is_scalar(&self) -> bool {
        matches!(
            self,
            RType::Numeric
                | RType::Character
                | RType::Logical
                | RType::Integer
                | RType::Null
        )
    }

    /// Check if this type is an array (arbitrary length)
    pub fn is_array(&self) -> bool {
        matches!(
            self,
            RType::NumericArray
                | RType::CharacterArray
                | RType::LogicalArray
                | RType::IntegerArray
        )
    }

    /// Get the element type for arrays
    pub fn element_type(&self) -> Option<RType> {
        match self {
            RType::NumericArray => Some(RType::Numeric),
            RType::CharacterArray => Some(RType::Character),
            RType::LogicalArray => Some(RType::Logical),
            RType::IntegerArray => Some(RType::Integer),
            RType::List(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Convert a scalar type to its array equivalent
    pub fn to_array(&self) -> Option<RType> {
        match self {
            RType::Numeric => Some(RType::NumericArray),
            RType::Character => Some(RType::CharacterArray),
            RType::Logical => Some(RType::LogicalArray),
            RType::Integer => Some(RType::IntegerArray),
            _ => None,
        }
    }

    /// Check if two types are compatible (one can be assigned to the other)
    pub fn is_compatible_with(&self, other: &RType) -> bool {
        // Any is compatible with everything
        if matches!(self, RType::Any) || matches!(other, RType::Any) {
            return true;
        }

        // Unknown is compatible with everything
        if matches!(self, RType::Unknown) || matches!(other, RType::Unknown) {
            return true;
        }

        // Exact type match
        if self == other {
            return true;
        }

        // Integer can be assigned to numeric (coercion)
        if matches!(self, RType::Integer) && matches!(other, RType::Numeric) {
            return true;
        }
        if matches!(self, RType::IntegerArray) && matches!(other, RType::NumericArray) {
            return true;
        }

        // Scalars can be promoted to arrays of the same type
        if let Some(array_type) = self.to_array() {
            if &array_type == other {
                return true;
            }
        }

        // Arrays can be demoted to scalars in some contexts (we'll be permissive for MVP)
        if let Some(element_type) = other.element_type() {
            if self == &element_type {
                return true;
            }
        }

        false
    }
}

impl std::fmt::Display for RType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RType::Null => write!(f, "null"),
            RType::Numeric => write!(f, "numeric"),
            RType::Character => write!(f, "character"),
            RType::Logical => write!(f, "logical"),
            RType::Integer => write!(f, "integer"),
            RType::NumericArray => write!(f, "numeric[]"),
            RType::CharacterArray => write!(f, "character[]"),
            RType::LogicalArray => write!(f, "logical[]"),
            RType::IntegerArray => write!(f, "integer[]"),
            RType::List(inner) => write!(f, "list[{}]", inner),
            RType::Record(fields) => {
                write!(f, "list{{")?;
                let mut first = true;
                for (name, ty) in fields {
                    if !first {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, ty)?;
                    first = false;
                }
                write!(f, "}}")
            }
            RType::Tuple(types) => {
                write!(f, "list(")?;
                for (i, ty) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", ty)?;
                }
                write!(f, ")")
            }
            RType::Function {
                params,
                named_params,
                return_type,
            } => {
                write!(f, "fn(")?;
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", param)?;
                }
                for (name, ty) in named_params {
                    if !params.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, ty)?;
                }
                write!(f, ") -> {}", return_type)
            }
            RType::Unknown => write!(f, "unknown"),
            RType::Any => write!(f, "any"),
        }
    }
}

/// A type annotation from source code
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAnnotation {
    pub r_type: RType,
    pub source_range: Option<SourceRange>,
}

/// Source location information
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_compatibility() {
        let numeric = RType::Numeric;
        let numeric_array = RType::NumericArray;
        let character = RType::Character;
        let any = RType::Any;
        let unknown = RType::Unknown;

        // Exact matches
        assert!(numeric.is_compatible_with(&numeric));
        assert!(numeric_array.is_compatible_with(&numeric_array));

        // Scalar to array promotion
        assert!(numeric.is_compatible_with(&numeric_array));

        // Integer can be assigned to numeric (coercion)
        assert!(RType::Integer.is_compatible_with(&RType::Numeric));
        assert!(RType::IntegerArray.is_compatible_with(&RType::NumericArray));

        // Different types are not compatible
        assert!(!numeric.is_compatible_with(&character));

        // Any is compatible with everything
        assert!(any.is_compatible_with(&numeric));
        assert!(numeric.is_compatible_with(&any));

        // Unknown is compatible with everything
        assert!(unknown.is_compatible_with(&numeric));
        assert!(numeric.is_compatible_with(&unknown));
    }

    #[test]
    fn test_type_display() {
        assert_eq!(RType::Numeric.to_string(), "numeric");
        assert_eq!(RType::NumericArray.to_string(), "numeric[]");
        assert_eq!(RType::List(Box::new(RType::Numeric)).to_string(), "list[numeric]");

        let mut record = HashMap::new();
        record.insert("name".to_string(), RType::Character);
        record.insert("age".to_string(), RType::Numeric);
        let record_type = RType::Record(record);
        let display = record_type.to_string();
        // Order might vary, but should contain both fields
        assert!(display.contains("name: character"));
        assert!(display.contains("age: numeric"));
        assert!(display.starts_with("list{"));
        assert!(display.ends_with("}"));
    }
}