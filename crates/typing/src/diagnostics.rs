use {
    crate::{
        infer::InferenceError,
        interner::{Interner, Symbol},
        types::{Atomic, CoreType, InferenceVariableId},
    },
    std::{collections::BTreeMap, fmt},
    tree_sitter::{Point, Range},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    pub fn render(&self, source: &str) -> String {
        if self.diagnostics.is_empty() {
            return "No diagnostics.\n".to_owned();
        }

        let mut rendered = String::new();

        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&diagnostic.render(source));
        }

        rendered
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub range: Range,
}

impl Diagnostic {
    pub fn syntax_error(range: Range, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: DiagnosticCode::SyntaxError,
            message: message.into(),
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
            InferenceError::ExpectedFunction {
                actual_type,
                range,
                expression_id: _,
            } => {
                let mut type_renderer = TypeRenderer::new(interner);
                (
                    *range,
                    format!(
                        "This expression is being called like a function, but it has type `{}`.",
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
                let mut type_renderer = TypeRenderer::new(interner);
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
                let mut type_renderer = TypeRenderer::new(interner);
                (
                    range.unwrap_or(fallback_range),
                    format!(
                        "This expression has type `{}`, but it needs to be `{}`.",
                        type_renderer.render(actual),
                        type_renderer.render(expected)
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
                    "This call uses named argument(s) `{}`, but the function accepts named parameter(s) `{}`.",
                    render_symbols(actual_parameters, interner),
                    render_symbols(expected_parameters, interner)
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
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => formatter.write_str("error"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    SyntaxError,
    TypeError,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SyntaxError => formatter.write_str("syntax-error"),
            Self::TypeError => formatter.write_str("type-error"),
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
    variable_names: BTreeMap<InferenceVariableId, String>,
    next_variable_index: usize,
}

impl<'a> TypeRenderer<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_names: BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn render(&mut self, core_type: &CoreType) -> String {
        self.render_core_type(core_type)
    }

    fn render_variable(&mut self, variable: InferenceVariableId) -> String {
        self.variable_name(variable).to_owned()
    }

    fn render_core_type(&mut self, core_type: &CoreType) -> String {
        match core_type {
            CoreType::Any => "any".to_owned(),
            CoreType::Unknown => "unknown".to_owned(),
            CoreType::Null => "null".to_owned(),
            CoreType::Scalar(atomic) => format!("scalar {}", render_atomic(*atomic)),
            CoreType::Vector(atomic) => format!("vector {}", render_atomic(*atomic)),
            CoreType::List(item_type) => {
                format!("list[{}]", self.render_core_type(item_type))
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
                format!("record{{{rendered_fields}}}")
            }
            CoreType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render_core_type(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("tuple({rendered_items})")
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
                        format!("{name}: {}", self.render_core_type(&parameter.value))
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
            CoreType::Variable(variable) => self.variable_name(*variable).to_owned(),
        }
    }

    fn variable_name(&mut self, variable: InferenceVariableId) -> &str {
        if !self.variable_names.contains_key(&variable) {
            let name = format!("type{}", self.next_variable_index + 1);
            self.next_variable_index += 1;
            self.variable_names.insert(variable, name);
        }

        self.variable_names
            .get(&variable)
            .map(String::as_str)
            .unwrap_or("type")
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
