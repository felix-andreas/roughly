use {
    std::fmt,
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
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SyntaxError => formatter.write_str("syntax-error"),
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
