use {
    crate::{
        interner::{Interner, Symbol},
        typecheck::{InferenceError, OperandExpectation, RECURSION_LIMIT, SubscriptKind},
        types::{Atomic, Constraint, CoreType, InferenceVariableId, TypeScheme},
    },
    std::{collections::BTreeMap, fmt},
    tree_sitter::{Point, Range},
};

pub type DocumentDiagnostics = (std::path::PathBuf, Vec<Diagnostic>);
pub type Diagnostics = Vec<Diagnostic>;

pub fn render_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.\n".to_owned();
    }

    let mut rendered = String::new();

    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str(&diagnostic.render(source));
    }

    rendered
}

pub fn render_core_type(interner: &Interner, core_type: &CoreType) -> String {
    let mut renderer = TypeRenderer::fixture(interner);
    renderer.render_core_type(core_type)
}

pub fn render_type_scheme(interner: &Interner, type_scheme: &TypeScheme) -> String {
    let mut renderer = TypeRenderer::fixture(interner);
    renderer.render_type_scheme(type_scheme)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub range: Range,
}

impl Diagnostic {
    pub fn lint_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::Lint,
            message: message.into(),
            range,
        }
    }

    pub fn lint_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Lint,
            message: message.into(),
            range,
        }
    }

    pub fn naming_warning(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: DiagnosticCode::Naming,
            message: message.into(),
            range,
        }
    }

    pub fn naming_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::Naming,
            message: message.into(),
            range,
        }
    }

    pub fn syntax_error(range: Range, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if !message.starts_with("Syntax Error: ") {
            message = format!("Syntax Error: {message}");
        }

        Self {
            severity: Severity::Error,
            code: DiagnosticCode::SyntaxError,
            message,
            range,
        }
    }

    pub fn type_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::TypeError,
            message: message.into(),
            range,
        }
    }

    pub fn annotation_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::AnnotationError,
            message: message.into(),
            range,
        }
    }

    pub fn missing_annotation_target(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment must be followed immediately by an expression.",
        )
    }

    pub fn invalid_annotation_syntax(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment contains invalid type syntax.",
        )
    }

    pub fn unknown_annotation_type(range: Range, name: impl Into<String>) -> Self {
        let name = name.into();
        Self::annotation_error(range, format!("I could not resolve type `{name}`."))
    }

    pub fn duplicate_annotation(range: Range) -> Self {
        Self::annotation_error(
            range,
            "This `#:` type comment cannot be followed by another `#:` type comment.",
        )
    }

    pub fn from_inference_error(
        error: &InferenceError,
        fallback_range: Range,
        interner: &Interner,
    ) -> Self {
        let (range, message) = match error {
            InferenceError::UnknownInferenceVariable(variable) => (
                fallback_range,
                format!(
                    "The type checker lost track of inference variable t{}.",
                    variable.0
                ),
            ),
            InferenceError::UnknownName {
                symbol,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (*range, format!("I could not find `{name}` in scope."))
            }
            InferenceError::AliasCycle {
                symbol,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (*range, format!("Type alias `{name}` expands in a cycle."))
            }
            InferenceError::ExpectedFunction {
                actual_type,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!(
                        "expected function, found `{}`",
                        type_renderer.render(actual_type)
                    ),
                )
            }
            InferenceError::OccursCheckFailed {
                variable,
                in_type,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                let variable_name = type_renderer.render_variable(*variable);
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "I cannot construct an infinite type: {variable_name} occurs inside `{}`.",
                        type_renderer.render(in_type)
                    ),
                )
            }
            InferenceError::TypeMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "expected `{}`, found `{}`",
                        type_renderer.render(expected),
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::UnresolvedAnnotationType { symbol } => {
                let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                (
                    fallback_range,
                    format!("I could not resolve type `{name}`."),
                )
            }
            InferenceError::ConstraintViolation {
                constraint,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                let expected_description = match constraint {
                    Constraint::Unconstrained => "a value",
                    Constraint::Numeric => "a numeric value (`integer` or `double`)",
                };
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "expected {expected_description}, found `{}`",
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::InvalidOperand {
                expected,
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                let expected_description = match expected {
                    OperandExpectation::Numeric => "a numeric value (`integer` or `double`)",
                    OperandExpectation::ScalarNumeric => {
                        "a scalar numeric value (`integer` or `double`)"
                    }
                    OperandExpectation::Logical => "a `logical` value",
                    OperandExpectation::Comparable => {
                        "a comparable value (numeric, `character`, or `logical`)"
                    }
                };
                (
                    *range,
                    format!(
                        "expected {expected_description}, found `{}`",
                        type_renderer.render(actual)
                    ),
                )
            }
            InferenceError::TupleLengthMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This tuple has {} item(s), but {} item(s) were expected.",
                    actual, expected
                ),
            ),
            InferenceError::MixedListElements {
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                "All elements in `list(...)` must be either all named or all unnamed.".to_owned(),
            ),
            InferenceError::RecordFieldMismatch {
                expected_fields,
                actual_fields,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This record has fields `{}`, but fields `{}` were expected.",
                    render_symbols(actual_fields, interner),
                    render_symbols(expected_fields, interner)
                ),
            ),
            InferenceError::FunctionArityMismatch {
                expected,
                actual,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This call passes {actual} positional argument(s), but the function accepts {expected}."
                ),
            ),
            InferenceError::NamedParameterMismatch {
                expected_parameters,
                actual_parameters,
                range,
                expression_id: _,
            } => (
                range.unwrap_or(fallback_range),
                format!(
                    "This call uses named argument(s) {}, but the function accepts named parameter(s) {}.",
                    render_symbols(actual_parameters, interner),
                    render_symbols(expected_parameters, interner)
                ),
            ),
            InferenceError::NotAList {
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!("expected a list, found `{}`", type_renderer.render(actual)),
                )
            }
            InferenceError::FieldDoesNotExist {
                field,
                container,
                range,
                expression_id: _,
            } => {
                let name = interner.resolve(*field).unwrap_or("<unknown>");
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!(
                        "field `{name}` does not exist in `{}`",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::PositionDoesNotExist {
                position,
                container,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!(
                        "position {position} does not exist in `{}`",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::NonLiteralSubscript {
                container,
                by,
                range,
                expression_id: _,
            } => {
                let detail = match by {
                    SubscriptKind::Position => "position",
                    SubscriptKind::FieldName => "field name",
                };
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!(
                        "cannot index `{}` without a statically known {detail}",
                        type_renderer.render(container)
                    ),
                )
            }
            InferenceError::UnsupportedSubset {
                actual,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::diagnostic(interner);
                (
                    *range,
                    format!("`[` is not supported on `{}`", type_renderer.render(actual)),
                )
            }
            InferenceError::RecursionLimitExceeded => (
                fallback_range,
                format!(
                    "This type is nested too deeply to check (more than {RECURSION_LIMIT} levels)."
                ),
            ),
        };

        Self::type_error(range, message)
    }

    pub fn render(&self, source: &str) -> String {
        let start = self.range.start_point;
        let end = self.range.end_point;
        let excerpt = excerpt_for_range(source, self.range);

        format!(
            "{severity}[{code}] {message}\n--> {start_line}:{start_column}-{end_line}:{end_column}\n{excerpt}\n",
            severity = self.severity,
            code = self.code,
            message = self.message,
            start_line = start.row + 1,
            start_column = start.column + 1,
            end_line = end.row + 1,
            end_column = end.column + 1,
            excerpt = excerpt,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => formatter.write_str("error"),
            Self::Warning => formatter.write_str("warning"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    Lint,
    Naming,
    SyntaxError,
    TypeError,
    AnnotationError,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lint => formatter.write_str("lint"),
            Self::Naming => formatter.write_str("naming"),
            Self::SyntaxError => formatter.write_str("syntax-error"),
            Self::TypeError => formatter.write_str("type-error"),
            Self::AnnotationError => formatter.write_str("annotation-error"),
        }
    }
}

pub fn point_label(point: Point) -> String {
    format!("{}:{}", point.row + 1, point.column + 1)
}

fn excerpt_for_range(source: &str, range: Range) -> String {
    let line_index = range.start_point.row;
    let line = source.lines().nth(line_index).unwrap_or("");

    if line.is_empty() {
        return "| <empty line>".to_owned();
    }

    format!("| {line}")
}

fn render_symbols(symbols: &[Symbol], interner: &Interner) -> String {
    if symbols.is_empty() {
        return "<none>".to_owned();
    }

    symbols
        .iter()
        .map(|symbol| format!("`{}`", interner.resolve(*symbol).unwrap_or("<unknown>")))
        .collect::<Vec<_>>()
        .join(", ")
}

struct TypeRenderer<'a> {
    interner: &'a Interner,
    variable_style: VariableRenderStyle,
    variable_names: BTreeMap<InferenceVariableId, String>,
    quantified_variable_names: BTreeMap<InferenceVariableId, String>,
    next_variable_index: usize,
}

#[derive(Clone, Copy)]
enum VariableRenderStyle {
    Diagnostic,
    Fixture,
}

impl<'a> TypeRenderer<'a> {
    fn diagnostic(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_style: VariableRenderStyle::Diagnostic,
            variable_names: BTreeMap::new(),
            quantified_variable_names: BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn fixture(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_style: VariableRenderStyle::Fixture,
            variable_names: BTreeMap::new(),
            quantified_variable_names: BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn render_type_scheme(&mut self, type_scheme: &TypeScheme) -> String {
        let quantified_names = type_scheme
            .quantified_variables
            .iter()
            .enumerate()
            .map(|(index, quantified)| {
                let name = quantified_variable_name(index);
                self.quantified_variable_names
                    .insert(quantified.variable, name.clone());
                match quantified.constraint {
                    Constraint::Unconstrained => name,
                    Constraint::Numeric => format!("{name}: numeric"),
                }
            })
            .collect::<Vec<_>>();
        let rendered_body = self.render_core_type(&type_scheme.body);

        if quantified_names.is_empty() {
            rendered_body
        } else {
            format!("<{}> {rendered_body}", quantified_names.join(", "))
        }
    }

    fn render_core_type(&mut self, core_type: &CoreType) -> String {
        match core_type {
            CoreType::Any => "Any".to_owned(),
            CoreType::Unknown => "Unknown".to_owned(),
            CoreType::Null => "NULL".to_owned(),
            CoreType::Nullable(inner_type) => {
                format!("{} | NULL", self.render_core_type(inner_type))
            }
            CoreType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            CoreType::Nominal(symbol, type_arguments) => {
                let name = self.interner.resolve(*symbol).unwrap_or("<unknown>");
                if type_arguments.is_empty() {
                    name.to_owned()
                } else {
                    let rendered_type_arguments = type_arguments
                        .iter()
                        .map(|type_argument| self.render_core_type(type_argument))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{name}<{rendered_type_arguments}>")
                }
            }
            CoreType::Vector(atomic) => format!("{}[]", render_atomic(*atomic)),
            CoreType::NamedVector(atomic) => format!("{}[named]", render_atomic(*atomic)),
            CoreType::List(item_type) => format!("list[{}]", self.render_core_type(item_type)),
            CoreType::NamedList(item_type) => {
                format!("list[named: {}]", self.render_core_type(item_type))
            }
            CoreType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render_core_type(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_fields}}}")
            }
            CoreType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render_core_type(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_items}}}")
            }
            CoreType::Function(function_type) => {
                let rendered_parameters = function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.render_core_type(parameter))
                    .collect::<Vec<_>>();
                let rendered_named_parameters = function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        let name = self.interner.resolve(parameter.name).unwrap_or("<unknown>");
                        let rendered_name = if parameter.optional {
                            format!("[{name}]")
                        } else {
                            name.to_owned()
                        };
                        format!(
                            "{rendered_name}: {}",
                            self.render_core_type(&parameter.value)
                        )
                    })
                    .collect::<Vec<_>>();
                let mut rendered_parts = rendered_parameters;
                rendered_parts.extend(rendered_named_parameters);
                format!(
                    "fn({}) -> {}",
                    rendered_parts.join(", "),
                    self.render_core_type(&function_type.return_type)
                )
            }
            CoreType::Variable(variable) => self.render_variable(*variable),
        }
    }

    fn render_variable(&mut self, variable: InferenceVariableId) -> String {
        if let Some(name) = self.quantified_variable_names.get(&variable) {
            return name.clone();
        }

        if !self.variable_names.contains_key(&variable) {
            let name = match self.variable_style {
                VariableRenderStyle::Diagnostic => format!("type{}", self.next_variable_index + 1),
                VariableRenderStyle::Fixture => format!("?{}", self.next_variable_index + 1),
            };
            self.next_variable_index += 1;
            self.variable_names.insert(variable, name);
        }

        self.variable_names
            .get(&variable)
            .cloned()
            .unwrap_or_else(|| match self.variable_style {
                VariableRenderStyle::Diagnostic => "type".to_owned(),
                VariableRenderStyle::Fixture => "?".to_owned(),
            })
    }

    fn render(&mut self, core_type: &CoreType) -> String {
        self.render_core_type(core_type)
    }
}

fn render_atomic(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}

fn quantified_variable_name(index: usize) -> String {
    const QUANTIFIED_NAMES: [&str; 7] = ["T", "U", "V", "W", "X", "Y", "Z"];

    QUANTIFIED_NAMES
        .get(index)
        .map(|name| (*name).to_owned())
        .unwrap_or_else(|| format!("T{}", index + 1))
}
