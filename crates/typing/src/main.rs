use tree_sitter::{Node, Parser, Tree};

fn main() {
    let mut parser = new_parser();

    let exprs = vec![r#"4 + 4"#, r#""foo" + 4"#, r#"fn(foo)"#];
    for expr in exprs {
        let tree = parse(&mut parser, expr, None);
        let result = check(tree.root_node());
        match result {
            Ok(typ) => eprintln!("expr: {expr}\ntype: {typ:?}\n"),
            Err((node, err)) => eprintln!("expr: {expr}\nnode: {node:?}\nerror: {}\n", err.0),
        }
    }

    // TODO: use miette
}

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
enum Type {
    // Any,
    Unknown,
    Integer,
    Float,
    Character,
}

#[derive(Debug)]
struct TypeError(String);

fn check(node: Node) -> Result<Type, (Node, TypeError)> {
    match node.kind() {
        "float" => Ok(Type::Float),
        "integer" => Ok(Type::Integer),
        "string" => Ok(Type::Character),
        "binary_operator" => {
            let lhs = node.child_by_field_name("lhs").unwrap();
            let rhs = node.child_by_field_name("rhs").unwrap();
            let lhs_type = check(lhs)?;
            let rhs_type = check(rhs)?;

            let operator = node.child_by_field_name("operator").unwrap();
            match operator.kind() {
                "+" => match (lhs_type, rhs_type) {
                    (Type::Integer, Type::Integer) => Ok(Type::Integer),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    (Type::Character, Type::Character) => Ok(Type::Character),
                    // todo: don't use wildecards here!
                    (a, b) => Err((node, TypeError(format!("Cannot add types {a:?} and {b:?}")))),
                },
                _ => Ok(Type::Unknown),
            }
        }
        "program" => node
            .children(&mut node.walk())
            .map(|child| check(child))
            .last()
            .unwrap_or(Ok(Type::Unknown)),
        _ => Ok(Type::Unknown),
    }
}
