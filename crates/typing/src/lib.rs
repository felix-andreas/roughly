use {
    std::collections::HashMap,
    tree_sitter::{Node, Parser, Range, Tree},
};

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("Error loading R parser");
    parser
}

pub fn parse(parser: &mut Parser, text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    parser.parse(text, maybe_tree).unwrap()
}

#[derive(Debug)]
pub enum Type {
    /// Explicit opt-out of type checking
    Any,
    /// Type could not be inferred
    Unknown,
    /// The null type
    Null,
    // Atomic scalar
    Scalar(Atomic),
    // Atomic vector
    Vector(Atomic),
    /// Homogeneous list of arbitrary length
    List(Box<Type>),
    /// Heterogeneous list of fixed length with named fields
    Record(HashMap<String, Type>),
    /// Heterogeneous list of fixed length with positional fields
    Tuple(Vec<Type>),
    /// Function type
    Function {
        params: Vec<Type>,
        named_params: HashMap<String, Type>,
        return_type: Box<Type>,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum Atomic {
    Logical,
    Integer,
    Double,
    Complex,
    Character,
    Raw,
}

#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub range: Range,
}

pub fn check(node: Node) -> Result<Type, Error> {
    match node.kind() {
        "float" => Ok(Type::Scalar(Atomic::Double)),
        "integer" => Ok(Type::Scalar(Atomic::Integer)),
        "complex" => Ok(Type::Scalar(Atomic::Complex)),
        "string" => Ok(Type::Scalar(Atomic::Character)),
        "false" | "true" => Ok(Type::Scalar(Atomic::Logical)),
        "binary_operator" => {
            let lhs = node.child_by_field_name("lhs").unwrap();
            let rhs = node.child_by_field_name("rhs").unwrap();
            let lhs_type = check(lhs)?;
            let rhs_type = check(rhs)?;

            let operator = node.child_by_field_name("operator").unwrap();
            match operator.kind() {
                "+" => match (lhs_type, rhs_type) {
                    (Type::Scalar(Atomic::Integer), Type::Scalar(Atomic::Integer)) => {
                        Ok(Type::Scalar(Atomic::Integer))
                    }
                    (Type::Scalar(Atomic::Double), Type::Scalar(Atomic::Double)) => {
                        Ok(Type::Scalar(Atomic::Double))
                    }
                    (Type::Scalar(Atomic::Character), Type::Scalar(Atomic::Character)) => {
                        Ok(Type::Scalar(Atomic::Character))
                    }
                    (Type::Scalar(Atomic::Integer), Type::Scalar(Atomic::Double))
                    | (Type::Scalar(Atomic::Double), Type::Scalar(Atomic::Integer)) => {
                        Ok(Type::Scalar(Atomic::Double))
                    }
                    (Type::Scalar(Atomic::Complex), Type::Scalar(Atomic::Integer))
                    | (Type::Scalar(Atomic::Integer), Type::Scalar(Atomic::Complex))
                    | (Type::Scalar(Atomic::Complex), Type::Scalar(Atomic::Double))
                    | (Type::Scalar(Atomic::Double), Type::Scalar(Atomic::Complex))
                    | (Type::Scalar(Atomic::Complex), Type::Scalar(Atomic::Complex)) => {
                        Ok(Type::Scalar(Atomic::Complex))
                    }
                    // todo: don't use wildecards here!
                    (a, b) => Err(Error {
                        message: format!("Cannot add types {a:?} and {b:?}"),
                        range: node.range(),
                    }),
                },
                "<-" => {
                    // TODO: add type
                    Ok(lhs_type)
                }
                _ => Ok(Type::Unknown),
            }
        }
        "program" | "block" => node
            .children(&mut node.walk())
            .map(|child| check(child))
            .last()
            .unwrap_or(Ok(Type::Unknown)),
        _ => Ok(Type::Unknown),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn setup(source: &str) -> Result<Type, Error> {
        let mut parser = new_parser();
        let tree = parse(&mut parser, source, None);
        let root = tree.root_node();
        check(root)
    }

    #[test]
    fn test_atomics() {
        assert!(matches!(
            setup("TRUE").unwrap(),
            Type::Scalar(Atomic::Logical)
        ));
        assert!(matches!(
            setup("FALSE").unwrap(),
            Type::Scalar(Atomic::Logical)
        ));
        assert!(matches!(
            setup("42L").unwrap(),
            Type::Scalar(Atomic::Integer)
        ));
        assert!(matches!(
            setup("3.14").unwrap(),
            Type::Scalar(Atomic::Double)
        ));
        assert!(matches!(
            setup("1+2i").unwrap(),
            Type::Scalar(Atomic::Complex)
        ));
        assert!(matches!(
            setup(r#""hello""#).unwrap(),
            Type::Scalar(Atomic::Character)
        ));
    }

    #[test]
    fn test_addition() {
        assert!(matches!(
            setup("1L + 2L").unwrap(),
            Type::Scalar(Atomic::Integer)
        ));
        assert!(matches!(
            setup("1.0 + 2.0").unwrap(),
            Type::Scalar(Atomic::Double)
        ));
        assert!(matches!(
            setup(r#""a" + "b""#).unwrap(),
            Type::Scalar(Atomic::Character)
        ));
    }

    #[test]
    fn test_add_incompatible_types() {
        assert!(setup(r#"1 + "a""#).is_err());
    }

    #[test]
    fn test_program_returns_last_type() {
        let ty = setup(r#"1\n2.0\n"foo""#).unwrap();
        assert!(matches!(ty, Type::Scalar(Atomic::Character)));
    }
}
