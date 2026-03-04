use {
    crate::{
        definition,
        index::{Item, ItemInfo, SymbolsMap},
        lsp_types::{
            ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
        },
        tree::{field, kind},
    },
    ropey::Rope,
    tree_sitter::{Node, Point, Tree},
};

pub fn get(
    position: Position,
    rope: &Rope,
    tree: &Tree,
    symbols_map: &impl SymbolsMap,
) -> Option<SignatureHelp> {
    let point = Point::new(position.line as usize, position.character as usize);
    let (call_node, active_parameter) = find_enclosing_call(tree, point)?;

    let function_node = call_node.child_by_field_id(field::FUNCTION)?;
    let function_name = rope.byte_slice(function_node.byte_range()).to_string();

    tracing::debug!(?function_name, ?active_parameter, "signature help");

    if let Some(signature) = signature_from_local(function_node, rope, &function_name) {
        return Some(build_signature_help(signature, active_parameter));
    }

    if let Some(signature) = signature_from_workspace(&function_name, symbols_map) {
        return Some(build_signature_help(signature, active_parameter));
    }

    None
}

fn find_enclosing_call(tree: &Tree, point: Point) -> Option<(Node<'_>, u32)> {
    let node = tree.root_node().descendant_for_point_range(point, point)?;

    let mut current = node;
    loop {
        if current.kind_id() == kind::ARGUMENTS {
            let parent = current.parent()?;
            if parent.kind_id() == kind::CALL {
                let active_parameter = count_active_parameter(current, point);
                return Some((parent, active_parameter));
            }
        }

        if current.kind_id() == kind::CALL {
            let arguments = current.child_by_field_id(field::ARGUMENTS)?;
            let active_parameter = count_active_parameter(arguments, point);
            return Some((current, active_parameter));
        }

        current = current.parent()?;
    }
}

fn count_active_parameter(arguments: Node, point: Point) -> u32 {
    let mut count = 0u32;

    let mut cursor = arguments.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();

            if child.start_position() >= point {
                break;
            }

            if child.kind_id() == kind::COMMA {
                count += 1;
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }

    count
}

fn signature_from_local(
    function_node: Node,
    rope: &Rope,
    function_name: &str,
) -> Option<(String, Vec<String>)> {
    if function_node.kind_id() != kind::IDENTIFIER {
        return None;
    }

    let definition_node = definition::find_previous_definition(function_node, rope, function_name)?;

    let assignment = definition_node.parent()?;
    if assignment.kind_id() != kind::BINARY_OPERATOR {
        return None;
    }

    let rhs = assignment.child_by_field_id(field::RHS)?;
    if rhs.kind_id() != kind::FUNCTION_DEFINITION {
        return None;
    }

    let parameters_node = rhs.child_by_field_id(field::PARAMETERS)?;
    let parameters = extract_parameters_from_node(parameters_node, rope);
    Some((function_name.to_string(), parameters))
}

fn signature_from_workspace(
    function_name: &str,
    symbols_map: &impl SymbolsMap,
) -> Option<(String, Vec<String>)> {
    let matches: Vec<(String, Vec<String>)> = symbols_map.filter_map(
        |_path, symbols| {
            let name = function_name;
            symbols
                .iter()
                .filter(move |item| item.name == name && item.info == ItemInfo::Function)
                .filter_map(|item| {
                    let parameters = parse_detail_parameters(item)?;
                    Some((item.name.clone(), parameters))
                })
        },
        1,
    );

    matches.into_iter().next()
}

fn extract_parameters_from_node(parameters_node: Node, rope: &Rope) -> Vec<String> {
    let mut parameters = vec![];
    let mut cursor = parameters_node.walk();

    for parameter in parameters_node.children_by_field_name("parameter", &mut cursor) {
        let name = parameter
            .child_by_field_id(field::NAME)
            .map(|name_node| rope.byte_slice(name_node.byte_range()).to_string());

        let default = parameter
            .child_by_field_id(field::DEFAULT)
            .map(|default_node| rope.byte_slice(default_node.byte_range()).to_string());

        match (name, default) {
            (Some(name), Some(default)) => parameters.push(format!("{name} = {default}")),
            (Some(name), None) => parameters.push(name),
            (None, _) => {
                let text = rope
                    .byte_slice(parameter.byte_range())
                    .to_string()
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    parameters.push(text);
                }
            }
        }
    }

    parameters
}

fn parse_detail_parameters(item: &Item) -> Option<Vec<String>> {
    let detail = item.detail.as_ref()?;
    let inner = detail.strip_prefix("fn(")?.strip_suffix(')')?;
    if inner.is_empty() {
        return Some(vec![]);
    }
    let mut parameters = vec![];
    let mut current = String::new();
    let mut depth = 0i32;
    for char in inner.chars() {
        match char {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(char);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(char);
            }
            ',' if depth == 0 => {
                parameters.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(char),
        }
    }
    let trailing = current.trim().to_string();
    if !trailing.is_empty() {
        parameters.push(trailing);
    }
    Some(parameters)
}

fn build_signature_help(
    (name, parameters): (String, Vec<String>),
    active_parameter: u32,
) -> SignatureHelp {
    let label = format!("{}({})", name, parameters.join(", "));

    let parameter_infos: Vec<ParameterInformation> = parameters
        .iter()
        .map(|param| ParameterInformation {
            label: ParameterLabel::Simple(param.clone()),
            documentation: None,
        })
        .collect();

    SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameter_infos),
            active_parameter: None,
        }],
        active_signature: Some(0),
        active_parameter: Some(active_parameter),
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::tree,
        indoc::indoc,
        std::{collections::HashMap, path::PathBuf},
    };

    fn setup(
        source: &str,
        line: usize,
        col: usize,
        workspace_items: &HashMap<PathBuf, Vec<Item>>,
    ) -> Option<SignatureHelp> {
        let rope = Rope::from_str(source);
        let tree = tree::parse(&mut tree::new_parser(), source, None);
        let position = Position::new(line as u32, col as u32);
        get(position, &rope, &tree, workspace_items)
    }

    #[test]
    fn local_function_basic() {
        let source = indoc! {r#"
            my_func <- function(x, y, z) {
                x + y + z
            }
            my_func(1, 2, 3)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 3, 9, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, "my_func(x, y, z)");
        assert_eq!(result.active_parameter, Some(0));
    }

    #[test]
    fn local_function_second_parameter() {
        let source = indoc! {r#"
            my_func <- function(x, y, z) {
                x + y + z
            }
            my_func(1, 2, 3)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 3, 12, &workspace).unwrap();
        assert_eq!(result.active_parameter, Some(1));
    }

    #[test]
    fn local_function_third_parameter() {
        let source = indoc! {r#"
            my_func <- function(x, y, z) {
                x + y + z
            }
            my_func(1, 2, 3)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 3, 15, &workspace).unwrap();
        assert_eq!(result.active_parameter, Some(2));
    }

    #[test]
    fn empty_arguments() {
        let source = indoc! {r#"
            my_func <- function(x, y) x + y
            my_func()
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 1, 8, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, "my_func(x, y)");
        assert_eq!(result.active_parameter, Some(0));
    }

    #[test]
    fn cursor_not_in_call() {
        let source = indoc! {r#"
            x <- 1
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 0, 3, &workspace);
        assert!(result.is_none());
    }

    #[test]
    fn nested_call_inner() {
        let source = indoc! {r#"
            f <- function(a) a
            g <- function(x, y) x + y
            g(f(1), 2)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 2, 4, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, "f(a)");
        assert_eq!(result.active_parameter, Some(0));
    }

    #[test]
    fn nested_call_outer() {
        let source = indoc! {r#"
            f <- function(a) a
            g <- function(x, y) x + y
            g(1, 2)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 2, 5, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, "g(x, y)");
        assert_eq!(result.active_parameter, Some(1));
    }

    #[test]
    fn function_with_defaults() {
        let source = indoc! {r#"
            f <- function(x, y = 10, z = "hello") x
            f(1)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 1, 2, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, r#"f(x, y = 10, z = "hello")"#);
        assert_eq!(result.signatures[0].parameters.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn workspace_function() {
        let mut workspace: HashMap<PathBuf, Vec<Item>> = HashMap::new();
        workspace.insert(
            PathBuf::from("/test/R/utils.R"),
            vec![Item::new(
                "helper".into(),
                Some("fn(a, b)".into()),
                crate::lsp_types::Range::default(),
                crate::lsp_types::Range::default(),
                None,
                ItemInfo::Function,
            )],
        );

        let source = "helper(1, 2)";
        let result = setup(source, 0, 7, &workspace).unwrap();
        assert_eq!(result.signatures[0].label, "helper(a, b)");
        assert_eq!(result.active_parameter, Some(0));
    }

    #[test]
    fn unknown_function_returns_none() {
        let source = "unknown_fn(1, 2)";
        let workspace = HashMap::new();

        let result = setup(source, 0, 11, &workspace);
        assert!(result.is_none());
    }

    #[test]
    fn parameter_count() {
        let source = indoc! {r#"
            f <- function(a, b, c, d, e) NULL
            f(1, 2, 3, 4, 5)
        "#};
        let workspace = HashMap::new();

        let result = setup(source, 1, 15, &workspace).unwrap();
        assert_eq!(result.active_parameter, Some(4));
    }

    #[test]
    fn workspace_function_with_defaults() {
        let mut workspace: HashMap<PathBuf, Vec<Item>> = HashMap::new();
        workspace.insert(
            PathBuf::from("/test/R/utils.R"),
            vec![Item::new(
                "read_data".into(),
                Some(r#"fn(path, header = TRUE, sep = ",")"#.into()),
                crate::lsp_types::Range::default(),
                crate::lsp_types::Range::default(),
                None,
                ItemInfo::Function,
            )],
        );

        let source = "read_data(\"file.csv\", )";
        let result = setup(source, 0, 21, &workspace).unwrap();
        assert_eq!(
            result.signatures[0].label,
            r#"read_data(path, header = TRUE, sep = ",")"#
        );
        assert_eq!(result.signatures[0].parameters.as_ref().unwrap().len(), 3);
        assert_eq!(result.active_parameter, Some(1));
    }

    #[test]
    fn workspace_function_with_nested_default() {
        let mut workspace: HashMap<PathBuf, Vec<Item>> = HashMap::new();
        workspace.insert(
            PathBuf::from("/test/R/utils.R"),
            vec![Item::new(
                "configure".into(),
                Some("fn(x, opts = list(a = 1, b = 2))".into()),
                crate::lsp_types::Range::default(),
                crate::lsp_types::Range::default(),
                None,
                ItemInfo::Function,
            )],
        );

        let source = "configure(1, )";
        let result = setup(source, 0, 13, &workspace).unwrap();
        assert_eq!(
            result.signatures[0].label,
            "configure(x, opts = list(a = 1, b = 2))"
        );
        assert_eq!(result.signatures[0].parameters.as_ref().unwrap().len(), 2);
        assert_eq!(result.active_parameter, Some(1));
    }
}
