//! The Roughly formatter: a preserving formatter over the lossless syntax
//! tree.
//!
//! The user's layout intent survives — single-line constructs stay single
//! line, multiline constructs keep their line structure ("hug" detection
//! reads the intent off the original bracket placement) — while spacing,
//! indentation, quoting, comment shape, and `#:` annotation blocks
//! normalize. `# fmt: skip` exempts one node byte-exactly, `# fmt: off` /
//! `# fmt: on` toggle whole regions, and `# fmt: skip-file` at the head
//! leaves the file untouched. A file with syntax errors refuses to format
//! (errors inside `#:` annotations do not refuse the file — the affected
//! block is preserved verbatim instead).

use syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub indent_width: usize,
    pub line_ending: LineEnding,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            indent_width: 2,
            line_ending: LineEnding::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineEnding {
    Auto,
    Lf,
    CrLf,
}

/// A syntax error refusing the format, at a zero-based line and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Syntax error: {} at line {}, column {}",
            self.message,
            self.line + 1,
            self.column + 1
        )
    }
}

impl std::error::Error for FormatError {}

pub fn format(source: &str, config: Config) -> Result<String, FormatError> {
    let parse = syntax::parse(source);
    let root = parse.syntax_node();
    let line_starts = line_starts(source);

    // Errors inside `#:` annotation regions do not refuse the file: the
    // affected block goes down the verbatim path (the oracle formatter never
    // even parsed annotations). Any other syntax error refuses.
    let annotation_ranges: Vec<TextRange> = root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::ANNOTATION)
        .map(|node| node.text_range())
        .collect();
    for error in parse.errors() {
        let inside_annotation = annotation_ranges
            .iter()
            .any(|range| range.contains_range(error.range) || range.contains(error.range.start()));
        if !inside_annotation {
            let offset = u32::from(error.range.start()) as usize;
            let line = line_of(&line_starts, offset);
            return Err(FormatError {
                message: error.message.clone(),
                line,
                column: offset - line_starts[line],
            });
        }
    }

    let line_ending = match config.line_ending {
        LineEnding::Auto => {
            if source
                .find('\n')
                .is_some_and(|at| source[..at].ends_with('\r'))
            {
                "\r\n"
            } else {
                "\n"
            }
        }
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
    };

    let mut formatter = Formatter {
        source,
        line_starts,
        error_ranges: parse.errors().iter().map(|error| error.range).collect(),
        indent: " ".repeat(config.indent_width),
        line_ending,
        out: String::with_capacity(source.len() * 3 / 2),
    };
    formatter.source_file(&root);
    Ok(formatter.out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    Skip,
    SkipFile,
    On,
    Off,
}

/// A node or token child, with trivia (whitespace and newlines) dropped —
/// the walk re-derives all layout. Comments, annotations, and semicolons
/// stay: containers place them.
type Element = SyntaxElement;

struct Formatter<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
    error_ranges: Vec<TextRange>,
    indent: String,
    line_ending: &'static str,
    out: String,
}

impl Formatter<'_> {
    // ---- layout primitives ----

    fn space(&mut self) {
        self.out.push(' ');
    }

    fn newline(&mut self, level: usize) {
        self.out.push_str(self.line_ending);
        for _ in 0..level {
            self.out.push_str(&self.indent);
        }
    }

    /// Between-sibling separation preserving the author's blank line (at
    /// most one): 1 or 2 line endings, then indentation.
    fn newlines_between(&mut self, previous: Option<&Element>, element: &Element, level: usize) {
        let count = previous.map_or(0, |previous| {
            let gap = self
                .line(element.text_range().start())
                .saturating_sub(self.line(previous.text_range().end()));
            gap.clamp(1, 2)
        });
        for _ in 0..count {
            self.out.push_str(self.line_ending);
        }
        for _ in 0..level {
            self.out.push_str(&self.indent);
        }
    }

    fn line(&self, offset: syntax::TextSize) -> usize {
        line_of(&self.line_starts, u32::from(offset) as usize)
    }

    fn same_line(&self, left: TextRange, right: TextRange) -> bool {
        self.line(left.end()) == self.line(right.start())
    }

    fn raw(&mut self, range: TextRange) {
        let start = u32::from(range.start()) as usize;
        let end = u32::from(range.end()) as usize;
        self.out.push_str(&self.source[start..end]);
    }

    // ---- shared walk helpers ----

    /// The non-trivia children (nodes, plus comment/semicolon/operator/
    /// punctuation tokens) in order.
    fn elements(node: &SyntaxNode) -> Vec<Element> {
        node.children_with_tokens()
            .filter(|element| {
                !matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
            })
            .collect()
    }

    fn is_comment(element: Option<&Element>) -> bool {
        element.is_some_and(|element| element.kind() == SyntaxKind::COMMENT)
    }

    fn comment_text(&self, element: &Element) -> &str {
        let range = element.text_range();
        &self.source[u32::from(range.start()) as usize..u32::from(range.end()) as usize]
    }

    fn directive(&self, element: &Element) -> Option<Directive> {
        if element.kind() != SyntaxKind::COMMENT {
            return None;
        }
        parse_directive(self.comment_text(element))
    }

    /// Whether a `# fmt: skip` comment exempts this element: a skip comment
    /// alone on the line directly above, or trailing on the element's last
    /// line.
    fn skipped_by_directive(&self, siblings: &[Element], index: usize) -> bool {
        let element = &siblings[index];
        let previous_is_skip = index
            .checked_sub(1)
            .map(|previous| &siblings[previous])
            .is_some_and(|previous| {
                self.directive(previous) == Some(Directive::Skip)
                    && index
                        .checked_sub(2)
                        .map(|before| &siblings[before])
                        .is_none_or(|before| {
                            !self.same_line(before.text_range(), previous.text_range())
                        })
            });
        let next_is_skip = siblings.get(index + 1).is_some_and(|next| {
            self.directive(next) == Some(Directive::Skip)
                && self.same_line(element.text_range(), next.text_range())
        });
        previous_is_skip || next_is_skip
    }

    /// `# fmt: skip` preserves the element byte-exactly, including the
    /// column of its first line: when only whitespace precedes it on its
    /// line, the indentation just emitted is replaced by the original
    /// leading whitespace, so column-aligned constructs survive.
    fn emit_skipped(&mut self, element: &Element) {
        let start = u32::from(element.text_range().start()) as usize;
        let line_start = self.line_starts[line_of(&self.line_starts, start)];
        let leading = &self.source[line_start..start];
        if leading.chars().all(char::is_whitespace) {
            let content_length = self.out.trim_end_matches(' ').len();
            if content_length == 0 || self.out[..content_length].ends_with(self.line_ending) {
                self.out.truncate(content_length);
                self.out.push_str(leading);
            }
        }
        self.raw(element.text_range());
    }

    fn element(&mut self, element: &Element, level: usize, make_multiline: bool) {
        match element {
            SyntaxElement::Node(node) => self.node(node, level, make_multiline),
            SyntaxElement::Token(token) => self.token(token, level),
        }
    }

    fn token(&mut self, token: &SyntaxToken, level: usize) {
        match token.kind() {
            SyntaxKind::COMMENT => self.comment(token),
            SyntaxKind::STRING => self.string(token),
            _ => self.raw(token.text_range()),
        }
        let _ = level;
    }

    fn node(&mut self, node: &SyntaxNode, level: usize, make_multiline: bool) {
        match node.kind() {
            SyntaxKind::LITERAL | SyntaxKind::NAME => {
                for element in Self::elements(node) {
                    self.element(&element, level, false);
                }
            }
            SyntaxKind::BINARY_EXPR => self.binary(node, level),
            SyntaxKind::UNARY_EXPR => self.unary(node, level),
            SyntaxKind::PAREN_EXPR => self.paren(node, level),
            SyntaxKind::BRACE_EXPR => self.braces(node, level, make_multiline),
            SyntaxKind::CALL_EXPR | SyntaxKind::SUBSET_EXPR | SyntaxKind::SUBSET2_EXPR => {
                self.call_like(node, level)
            }
            SyntaxKind::ARGUMENT_LIST => self.argument_list(node, level, false),
            SyntaxKind::PARAMETER_LIST => self.argument_list(node, level, false),
            SyntaxKind::DOLLAR_EXPR | SyntaxKind::AT_EXPR => self.field_access(node, level),
            SyntaxKind::NAMESPACE_EXPR => {
                for element in Self::elements(node) {
                    self.element(&element, level, false);
                }
            }
            SyntaxKind::FUNCTION_DEF => self.function_definition(node, level),
            SyntaxKind::IF_EXPR => self.if_expression(node, level, make_multiline),
            SyntaxKind::FOR_EXPR => self.for_expression(node, level),
            SyntaxKind::WHILE_EXPR => self.while_expression(node, level),
            SyntaxKind::REPEAT_EXPR => self.repeat_expression(node, level),
            SyntaxKind::BREAK_EXPR | SyntaxKind::NEXT_EXPR => {
                for element in Self::elements(node) {
                    self.element(&element, level, false);
                }
            }
            SyntaxKind::ANNOTATION => self.annotation(node, level),
            _ => {
                // Every other node (argument, parameter, type nodes reached
                // outside an annotation) prints its children with canonical
                // single spacing decisions made by the caller.
                for element in Self::elements(node) {
                    self.element(&element, level, false);
                }
            }
        }
    }

    // ---- the top level ----

    fn source_file(&mut self, root: &SyntaxNode) {
        let elements = Self::elements(root);

        if elements
            .first()
            .is_some_and(|first| self.directive(first) == Some(Directive::SkipFile))
        {
            self.out.push_str(self.source);
            return;
        }

        self.item_sequence(&elements, 0);

        // An empty file stays empty; anything else ends with one newline.
        if !elements.is_empty() || !self.source.is_empty() {
            self.newline(0);
        }
    }

    /// A sequence of statements at one level: the top level, or a multiline
    /// brace body. Handles `# fmt: off` regions, `# fmt: skip`, same-line
    /// trailing comments, and semicolon-separated statements (normalized to
    /// newlines).
    fn item_sequence(&mut self, elements: &[Element], level: usize) {
        let mut enabled = true;
        let mut pending_directive = None;
        let mut previous: Option<&Element> = None;
        let mut first = true;

        for (index, element) in elements.iter().enumerate() {
            match pending_directive {
                Some(Directive::On) => enabled = true,
                Some(Directive::Off) => enabled = false,
                _ => {}
            }
            pending_directive = self.directive(element);

            if element.kind() == SyntaxKind::SEMICOLON {
                continue;
            }

            if !enabled {
                // A `# fmt: off` region is preserved byte-exactly; only the
                // directive comments themselves are formatted.
                let start = previous.map_or_else(
                    || u32::from(element.text_range().start()) as usize,
                    |previous| u32::from(previous.text_range().end()) as usize,
                );
                let end = u32::from(element.text_range().end()) as usize;
                if pending_directive.is_some() {
                    let element_start = u32::from(element.text_range().start()) as usize;
                    self.out.push_str(&self.source[start..element_start]);
                    self.element(element, level, false);
                } else {
                    self.out.push_str(&self.source[start..end]);
                }
                previous = Some(element);
                first = false;
                continue;
            }

            let trailing_comment =
                matches!(element.kind(), SyntaxKind::COMMENT | SyntaxKind::ANNOTATION)
                    && previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    });
            if trailing_comment {
                self.space();
            } else if first {
                for _ in 0..level {
                    self.out.push_str(&self.indent);
                }
            } else {
                self.newlines_between(previous, element, level);
            }

            if self.skipped_by_directive(elements, index) {
                self.emit_skipped(element);
            } else {
                self.element(element, level, false);
            }
            previous = Some(element);
            first = false;
        }
    }

    // ---- comments and strings ----

    fn comment(&mut self, token: &SyntaxToken) {
        let raw = self
            .comment_text(&SyntaxElement::Token(token.clone()))
            .trim_end()
            .to_owned();
        let mut chars = raw.chars();
        let _ = chars.next();
        match chars.next() {
            Some(char @ ('\'' | '*')) => match chars.next() {
                Some(' ') | None => self.out.push_str(&raw),
                Some(_) if char == '\'' && chars.clone().any(|c| c == '\'') => {
                    self.out.push_str(&raw)
                }
                Some(other) => {
                    self.out.push('#');
                    self.out.push(char);
                    self.out.push(' ');
                    self.out.push(other);
                    self.out.extend(chars);
                }
            },
            // `#!` shebangs, `#|` Quarto/knitr cell options, `##`, and
            // already-spaced comments stay untouched.
            Some('#' | '!' | '|' | ' ') | None => self.out.push_str(&raw),
            Some(other) => {
                self.out.push_str("# ");
                self.out.push(other);
                self.out.extend(chars);
            }
        }
    }

    fn string(&mut self, token: &SyntaxToken) {
        let text = {
            let range = token.text_range();
            &self.source[u32::from(range.start()) as usize..u32::from(range.end()) as usize]
        };
        // Raw strings are preserved byte-for-byte: re-quoting would corrupt
        // embedded quotes and backslashes.
        if matches!(text.chars().next(), Some('r' | 'R')) {
            self.out.push_str(text);
            return;
        }
        let content = &text[1..text.len().saturating_sub(1)];
        let mut all_quotes_escaped = true;
        let mut previous_was_escape = false;
        for char in content.chars() {
            if char == '"' {
                all_quotes_escaped &= previous_was_escape;
            }
            previous_was_escape = char == '\\' && !previous_was_escape;
        }
        let quote = if all_quotes_escaped { '"' } else { '\'' };
        self.out.push(quote);
        self.out.push_str(content);
        self.out.push(quote);
    }

    // ---- operators ----

    fn binary(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let operator = elements.iter().position(|element| {
            element
                .as_token()
                .is_some_and(|token| is_operator(token.kind()))
        });
        let Some(operator_index) = operator else {
            for element in &elements {
                self.element(element, level, false);
            }
            return;
        };
        let operator_kind = elements[operator_index].kind();
        let has_spacing = !matches!(operator_kind, SyntaxKind::COLON | SyntaxKind::CARET);
        let break_after_operator = elements
            .get(operator_index + 1..)
            .and_then(|rest| rest.iter().find(|element| element.as_node().is_some()))
            .is_some_and(|rhs| {
                !self.same_line(elements[operator_index].text_range(), rhs.text_range())
            });

        let mut seen_operator = false;
        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                        self.element(element, level, false);
                    } else {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    }
                }
                kind if element.as_token().is_some() && is_operator(kind) => {
                    if Self::is_comment(previous) {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        if has_spacing {
                            self.space();
                        }
                        self.element(element, level, false);
                    }
                    seen_operator = true;
                }
                _ => {
                    if !seen_operator {
                        self.element(element, level, false);
                    } else if break_after_operator || Self::is_comment(previous) {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        if has_spacing {
                            self.space();
                        }
                        self.element(element, level, false);
                    }
                }
            }
            previous = Some(element);
        }
    }

    fn unary(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let operand_is_name = elements
            .iter()
            .rev()
            .find(|element| element.as_node().is_some())
            .is_some_and(|operand| operand.kind() == SyntaxKind::NAME);
        let has_space = elements
            .first()
            .is_some_and(|operator| operator.kind() == SyntaxKind::TILDE)
            && !operand_is_name;

        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                        self.element(element, level, false);
                    } else {
                        self.newlines_between(previous, element, level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    }
                }
                _ if element.as_node().is_some() => {
                    if Self::is_comment(previous) {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        if previous.is_some() && has_space {
                            self.space();
                        }
                        self.element(element, level, false);
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn field_access(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let operator = elements
            .iter()
            .position(|element| matches!(element.kind(), SyntaxKind::DOLLAR | SyntaxKind::AT));
        let is_multiline = operator.is_some_and(|operator_index| {
            elements
                .get(operator_index + 1..)
                .and_then(|rest| rest.first())
                .is_some_and(|rhs| {
                    !self.same_line(
                        elements[..operator_index]
                            .iter()
                            .next_back()
                            .map_or(elements[operator_index].text_range(), |lhs| {
                                lhs.text_range()
                            }),
                        rhs.text_range(),
                    )
                })
        });

        let mut seen_operator = false;
        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::DOLLAR | SyntaxKind::AT => {
                    self.element(element, level, false);
                    seen_operator = true;
                }
                _ => {
                    if seen_operator && is_multiline {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        self.element(element, level, false);
                    }
                }
            }
            previous = Some(element);
        }
    }

    // ---- groups ----

    fn paren(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let close = elements
            .iter()
            .rposition(|element| element.kind() == SyntaxKind::R_PAREN);
        let hug = close.is_none_or(|close_index| {
            close_index
                .checked_sub(1)
                .map(|body| &elements[body])
                .is_none_or(|body| {
                    body.kind() != SyntaxKind::COMMENT
                        && self.line(body.text_range().start())
                            == self.line(node.text_range().start())
                })
        });

        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::L_PAREN => self.element(element, level, false),
                SyntaxKind::R_PAREN => {
                    if !hug {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                    }
                    self.element(element, level, false);
                }
                _ => {
                    if !hug {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        self.element(element, level, false);
                    }
                }
            }
            previous = Some(element);
        }
    }

    fn braces(&mut self, node: &SyntaxNode, level: usize, make_multiline: bool) {
        let elements = Self::elements(node);
        let close = elements
            .iter()
            .rposition(|element| element.kind() == SyntaxKind::R_BRACE);
        let hug = close.is_none_or(|close_index| {
            close_index
                .checked_sub(1)
                .map(|body| &elements[body])
                .is_none_or(|body| {
                    body.kind() != SyntaxKind::COMMENT
                        && self.line(body.text_range().start())
                            == self.line(node.text_range().start())
                        && self.line(body.text_range().end())
                            == self.line(node.text_range().start())
                })
        });
        let is_multiline = !hug || make_multiline;
        let body_elements: Vec<Element> = elements
            .iter()
            .filter(|element| !matches!(element.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE))
            .cloned()
            .collect();
        let is_empty = body_elements.is_empty();

        self.out.push('{');
        if is_multiline {
            let saved = std::mem::take(&mut self.out);
            self.item_sequence(&body_elements, level + 1);
            let body = std::mem::replace(&mut self.out, saved);
            if !body.is_empty() {
                self.newline(level + 1);
                let trimmed_newline = self.out.len() - self.indent.len() * (level + 1);
                self.out.truncate(trimmed_newline);
                self.out.push_str(&self.indent.repeat(level + 1));
                self.out.push_str(&body);
            }
            self.newline(level);
            self.out.push('}');
        } else {
            let mut first = true;
            let mut previous: Option<&Element> = None;
            for element in &body_elements {
                if element.kind() == SyntaxKind::SEMICOLON {
                    previous = Some(element);
                    continue;
                }
                if element.kind() == SyntaxKind::COMMENT {
                    self.space();
                    self.element(element, level, false);
                    previous = Some(element);
                    continue;
                }
                if first {
                    self.space();
                } else {
                    self.out.push_str("; ");
                }
                self.element(element, level, false);
                first = false;
                previous = Some(element);
            }
            let _ = previous;
            if !is_empty {
                self.space();
            }
            self.out.push('}');
        }
    }

    // ---- calls and argument lists ----

    fn call_like(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let additional_indent = elements.first().is_some_and(|target| {
            matches!(target.kind(), SyntaxKind::DOLLAR_EXPR | SyntaxKind::AT_EXPR)
                && target.as_node().is_some_and(|target| {
                    let parts = Self::elements(target);
                    let operator = parts.iter().position(|part| {
                        matches!(part.kind(), SyntaxKind::DOLLAR | SyntaxKind::AT)
                    });
                    operator.is_some_and(|operator_index| {
                        parts.get(operator_index + 1).is_some_and(|rhs| {
                            parts.first().is_some_and(|lhs| {
                                !self.same_line(lhs.text_range(), rhs.text_range())
                            })
                        })
                    })
                })
        });

        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                        self.element(element, level, false);
                    } else {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    }
                }
                SyntaxKind::ARGUMENT_LIST => {
                    if additional_indent {
                        self.argument_list(
                            element.as_node().expect("argument list is a node"),
                            level + 1,
                            true,
                        );
                    } else {
                        self.argument_list(
                            element.as_node().expect("argument list is a node"),
                            level,
                            false,
                        );
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn argument_list(&mut self, node: &SyntaxNode, level: usize, _skip_first_indent: bool) {
        let elements = Self::elements(node);
        let close = elements.iter().rposition(|element| {
            matches!(element.kind(), SyntaxKind::R_PAREN | SyntaxKind::R_BRACKET)
        });
        let list_range = node.text_range();
        let is_multiline = self.line(list_range.start()) != self.line(list_range.end());
        let arguments: Vec<&Element> = elements
            .iter()
            .filter(|element| {
                element.kind() == SyntaxKind::ARGUMENT || element.kind() == SyntaxKind::PARAMETER
            })
            .collect();
        let is_empty = arguments
            .iter()
            .all(|argument| argument.text_range().is_empty());

        let close_previous = close
            .and_then(|close_index| close_index.checked_sub(1))
            .map(|index| &elements[index]);
        let trailing_space =
            close_previous.is_none_or(|previous| previous.kind() == SyntaxKind::COMMA);
        let hug = close_previous.is_none_or(|close_previous| {
            close_previous.kind() != SyntaxKind::COMMENT
                && arguments.iter().any(|argument| {
                    !argument.text_range().is_empty()
                        && self.line(argument.text_range().start()) == self.line(list_range.start())
                        && self.line(argument.text_range().end())
                            == self.line(close_previous.text_range().end())
                })
        });

        let mut argument_index = 0usize;
        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::L_PAREN | SyntaxKind::L_BRACKET | SyntaxKind::L_BRACKET2 => {
                    self.element(element, level, false);
                }
                SyntaxKind::R_PAREN | SyntaxKind::R_BRACKET => {
                    if is_multiline {
                        if !hug && !is_empty {
                            self.newline(level);
                        }
                    } else if trailing_space && !is_empty {
                        self.space();
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                        self.element(element, level, false);
                    } else {
                        self.newlines_between(previous, element, level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    }
                }
                SyntaxKind::COMMA => {
                    if is_multiline
                        && !hug
                        && (previous.is_none_or(|previous| {
                            matches!(previous.kind(), SyntaxKind::COMMENT | SyntaxKind::COMMA)
                                || previous.text_range().is_empty()
                        }))
                    {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                    } else if previous.is_some_and(|previous| {
                        previous.kind() == SyntaxKind::ARGUMENT
                            && previous.as_node().is_some_and(|argument| {
                                argument
                                    .children_with_tokens()
                                    .last()
                                    .is_some_and(|last| last.kind() == SyntaxKind::EQ)
                            })
                    }) {
                        self.space();
                    }
                    self.out.push(',');
                }
                SyntaxKind::ARGUMENT | SyntaxKind::PARAMETER => {
                    let empty = element.text_range().is_empty();
                    if !empty {
                        if argument_index == 0 {
                            if is_multiline && !hug {
                                self.newline(level);
                                self.out.push_str(&self.indent);
                            }
                        } else if is_multiline && !hug {
                            self.newlines_between(previous, element, level);
                            self.out.push_str(&self.indent);
                        } else {
                            self.space();
                        }
                        let argument_level = if is_multiline && !hug {
                            level + 1
                        } else {
                            level
                        };
                        self.argument(
                            element.as_node().expect("argument is a node"),
                            argument_level,
                        );
                    }
                    argument_index += 1;
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn argument(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let mut previous: Option<&Element> = None;
        for element in &elements {
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::EQ => self.out.push_str(" ="),
                _ => {
                    if Self::is_comment(previous) {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        if previous.is_some_and(|previous| previous.kind() == SyntaxKind::EQ) {
                            self.space();
                        }
                        self.element(element, level, false);
                    }
                }
            }
            previous = Some(element);
        }
    }

    // ---- definitions and control flow ----

    fn function_definition(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let range = node.text_range();
        let is_multiline = self.line(range.start()) != self.line(range.end());
        let head = elements.first();
        let body = elements.iter().rev().find(|element| {
            element.as_node().is_some() && element.kind() != SyntaxKind::PARAMETER_LIST
        });
        let is_same_line = match (head, body) {
            (Some(head), Some(body)) => {
                self.line(head.text_range().start()) == self.line(body.text_range().start())
            }
            _ => true,
        };
        let body_range = body.map(|body| body.text_range());

        let mut previous: Option<&Element> = None;
        for element in &elements {
            let is_body = body_range == Some(element.text_range()) && element.as_node().is_some();
            match element.kind() {
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::PARAMETER_LIST => {
                    if Self::is_comment(previous) {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                }
                _ if is_body => {
                    if Self::is_comment(previous) {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    if is_multiline {
                        if element.kind() == SyntaxKind::BRACE_EXPR || is_same_line {
                            self.element(element, level, true);
                        } else {
                            self.wrap_braces(element, level);
                        }
                    } else {
                        self.element(element, level, false);
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn wrap_braces(&mut self, element: &Element, level: usize) {
        self.out.push('{');
        self.newline(level + 1);
        self.element(element, level + 1, false);
        self.newline(level);
        self.out.push('}');
    }

    fn if_expression(&mut self, node: &SyntaxNode, level: usize, make_multiline: bool) {
        let elements = Self::elements(node);
        let range = node.text_range();
        let is_multiline = make_multiline || self.line(range.start()) != self.line(range.end());

        let open = elements
            .iter()
            .position(|element| element.kind() == SyntaxKind::L_PAREN);
        let condition = open.and_then(|open_index| {
            elements
                .get(open_index + 1..)
                .and_then(|rest| rest.iter().find(|element| element.as_node().is_some()))
        });
        let hug = match (open, condition) {
            (Some(open_index), Some(condition)) => {
                let open_element = &elements[open_index];
                let condition_position = elements
                    .iter()
                    .position(|element| element.text_range() == condition.text_range())
                    .unwrap_or(open_index + 1);
                let no_comments = !Self::is_comment(
                    condition_position
                        .checked_sub(1)
                        .map(|index| &elements[index]),
                ) && !Self::is_comment(elements.get(condition_position + 1));
                self.same_line(open_element.text_range(), condition.text_range()) && no_comments
            }
            _ => true,
        };

        let close = elements
            .iter()
            .position(|element| element.kind() == SyntaxKind::R_PAREN);
        let mut seen_close = false;
        let mut indent_comments = false;
        let mut previous: Option<&Element> = None;
        let mut is_alternative = false;
        for (index, element) in elements.iter().enumerate() {
            let previous_is_comment = Self::is_comment(previous);
            match element.kind() {
                SyntaxKind::IF_KW => self.element(element, level, false),
                SyntaxKind::ELSE_KW => {
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    self.element(element, level, false);
                    is_alternative = true;
                }
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                        if indent_comments {
                            self.out.push_str(&self.indent);
                        }
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::L_PAREN => {
                    indent_comments = true;
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::R_PAREN => {
                    indent_comments = false;
                    if !hug {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                    seen_close = true;
                }
                _ if element.as_node().is_some() => {
                    if !seen_close && Some(index) != close {
                        // The condition.
                        if !hug {
                            self.newline(level);
                            self.out.push_str(&self.indent);
                            self.element(element, level + 1, false);
                        } else {
                            self.element(element, level, false);
                        }
                    } else {
                        // A branch.
                        if previous_is_comment {
                            self.newline(level);
                        } else {
                            self.space();
                        }
                        if is_multiline {
                            if element.kind() == SyntaxKind::BRACE_EXPR
                                || (element.kind() == SyntaxKind::IF_EXPR && is_alternative)
                            {
                                self.element(element, level, true);
                            } else {
                                self.wrap_braces(element, level);
                            }
                        } else {
                            self.element(element, level, false);
                        }
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn for_expression(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let open = elements
            .iter()
            .find(|element| element.kind() == SyntaxKind::L_PAREN);
        let variable = elements
            .iter()
            .find(|element| element.kind() == SyntaxKind::NAME);
        let in_index = elements
            .iter()
            .position(|element| element.kind() == SyntaxKind::IN_KW);
        let sequence = in_index.and_then(|index| {
            elements
                .get(index + 1..)
                .and_then(|rest| rest.iter().find(|element| element.as_node().is_some()))
        });

        let condition_is_multiline = match (open, sequence) {
            (Some(open), Some(sequence)) => {
                !self.same_line(open.text_range(), sequence.text_range())
            }
            _ => false,
        };
        let loop_header_is_multiline = match (variable, sequence) {
            (Some(variable), Some(sequence)) => {
                !self.same_line(variable.text_range(), sequence.text_range())
            }
            _ => false,
        };
        let sequence_range = sequence.map(|sequence| sequence.text_range());
        let variable_range = variable.map(|variable| variable.text_range());

        let mut seen_close = false;
        let mut indent_comments = false;
        let mut previous: Option<&Element> = None;
        for element in &elements {
            let previous_is_comment = Self::is_comment(previous);
            match element.kind() {
                SyntaxKind::FOR_KW => self.element(element, level, false),
                SyntaxKind::IN_KW => {
                    if previous_is_comment || loop_header_is_multiline {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                    } else {
                        self.space();
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                        if indent_comments {
                            self.out.push_str(&self.indent);
                        }
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::L_PAREN => {
                    indent_comments = true;
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::R_PAREN => {
                    indent_comments = false;
                    if previous_is_comment || condition_is_multiline {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                    seen_close = true;
                }
                SyntaxKind::NAME if Some(element.text_range()) == variable_range && !seen_close => {
                    if previous_is_comment || condition_is_multiline {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        self.element(element, level, false);
                    }
                }
                _ if element.as_node().is_some()
                    && Some(element.text_range()) == sequence_range =>
                {
                    if previous_is_comment {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        self.space();
                        if condition_is_multiline {
                            self.element(element, level + 1, false);
                        } else {
                            self.element(element, level, false);
                        }
                    }
                }
                _ if element.as_node().is_some() && seen_close => {
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    if element.kind() == SyntaxKind::BRACE_EXPR {
                        self.element(element, level, true);
                    } else {
                        self.wrap_braces(element, level);
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn while_expression(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let open = elements
            .iter()
            .position(|element| element.kind() == SyntaxKind::L_PAREN);
        let condition = open.and_then(|open_index| {
            elements
                .get(open_index + 1..)
                .and_then(|rest| rest.iter().find(|element| element.as_node().is_some()))
        });
        let hug = match (open, condition) {
            (Some(open_index), Some(condition)) => {
                let condition_position = elements
                    .iter()
                    .position(|element| element.text_range() == condition.text_range())
                    .unwrap_or(open_index + 1);
                let no_comments = !Self::is_comment(
                    condition_position
                        .checked_sub(1)
                        .map(|index| &elements[index]),
                ) && !Self::is_comment(elements.get(condition_position + 1));
                self.same_line(elements[open_index].text_range(), condition.text_range())
                    && no_comments
            }
            _ => true,
        };

        let mut seen_close = false;
        let mut indent_comments = false;
        let mut previous: Option<&Element> = None;
        for element in &elements {
            let previous_is_comment = Self::is_comment(previous);
            match element.kind() {
                SyntaxKind::WHILE_KW => self.element(element, level, false),
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                        if indent_comments {
                            self.out.push_str(&self.indent);
                        }
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::L_PAREN => {
                    indent_comments = true;
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    self.element(element, level, false);
                }
                SyntaxKind::R_PAREN => {
                    indent_comments = false;
                    if !hug {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                    seen_close = true;
                }
                _ if element.as_node().is_some() && !seen_close => {
                    if !hug {
                        self.newline(level);
                        self.out.push_str(&self.indent);
                        self.element(element, level + 1, false);
                    } else {
                        self.element(element, level, false);
                    }
                }
                _ if element.as_node().is_some() => {
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    if element.kind() == SyntaxKind::BRACE_EXPR {
                        self.element(element, level, true);
                    } else {
                        self.wrap_braces(element, level);
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    fn repeat_expression(&mut self, node: &SyntaxNode, level: usize) {
        let elements = Self::elements(node);
        let mut previous: Option<&Element> = None;
        for element in &elements {
            let previous_is_comment = Self::is_comment(previous);
            match element.kind() {
                SyntaxKind::REPEAT_KW => self.element(element, level, false),
                SyntaxKind::COMMENT => {
                    if previous.is_some_and(|previous| {
                        self.same_line(previous.text_range(), element.text_range())
                    }) {
                        self.space();
                    } else {
                        self.newline(level);
                    }
                    self.element(element, level, false);
                }
                _ if element.as_node().is_some() => {
                    if previous_is_comment {
                        self.newline(level);
                    } else {
                        self.space();
                    }
                    if element.kind() == SyntaxKind::BRACE_EXPR {
                        self.element(element, level, true);
                    } else {
                        self.wrap_braces(element, level);
                    }
                }
                _ => self.element(element, level, false),
            }
            previous = Some(element);
        }
    }

    // ---- `#:` annotations ----

    /// Renders one stitched annotation region. When the block parsed cleanly
    /// it is re-rendered canonically (author line breaks kept, token spacing
    /// and bracket hugging normalized); a block with a parse error inside is
    /// preserved line-by-line verbatim, apart from a single space after
    /// every `#:`.
    fn annotation(&mut self, node: &SyntaxNode, level: usize) {
        let range = node.text_range();
        let has_error = self
            .error_ranges
            .iter()
            .any(|error| range.contains_range(*error) || range.contains(error.start()));

        let lines = if has_error {
            None
        } else {
            self.render_annotation(node)
        };
        let lines = lines.unwrap_or_else(|| self.verbatim_annotation(node));

        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                self.newline(level);
            }
            self.out.push_str(line);
        }
    }

    /// The annotation region's original lines, kept verbatim after the `#:`.
    fn verbatim_annotation(&self, node: &SyntaxNode) -> Vec<String> {
        let range = node.text_range();
        let text = &self.source[u32::from(range.start()) as usize..u32::from(range.end()) as usize];
        text.lines()
            .map(|line| {
                let body = line
                    .trim_start()
                    .strip_prefix("#:")
                    .unwrap_or(line)
                    .trim_end();
                match body.chars().next() {
                    None => "#:".to_owned(),
                    Some(character) if character.is_whitespace() => format!("#:{body}"),
                    Some(_) => format!("#: {}", body.trim_start()),
                }
            })
            .collect()
    }

    /// Canonical annotation rendering: tokens re-spaced per the type
    /// notation's rules, author line breaks kept, bracket hug bits read off
    /// the original layout (an opener ending its line expands: its closer
    /// gets its own line at the opener's indent; anything else hugs).
    fn render_annotation(&self, node: &SyntaxNode) -> Option<Vec<String>> {
        let mut line_tokens: Vec<Vec<AnnotationToken>> = Vec::new();
        let mut markers = 0usize;
        let mut current_line = usize::MAX;
        for element in node.descendants_with_tokens() {
            let Some(token) = element.as_token() else {
                continue;
            };
            match token.kind() {
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => continue,
                SyntaxKind::ANNOTATION_MARKER => {
                    markers += 1;
                    let line = self.line(token.text_range().start());
                    if line != current_line {
                        line_tokens.push(Vec::new());
                        current_line = line;
                    }
                    continue;
                }
                _ => {}
            }
            let line = self.line(token.text_range().start());
            if line != current_line {
                line_tokens.push(Vec::new());
                current_line = line;
            }
            let kind = annotation_token_kind(token.kind(), token.text())?;
            let last = line_tokens.last_mut().expect("marker precedes tokens");
            // Directive names lex as `@` plus adjacent word/`-` tokens; glue
            // them back into one display token.
            if kind == AnnotationTokenKind::Word
                && last.last().is_some_and(|previous| {
                    previous.kind == AnnotationTokenKind::Directive
                        && previous.end == u32::from(token.text_range().start())
                })
            {
                let previous = last.last_mut().expect("directive precedes");
                previous.text.push_str(token.text());
                previous.end = u32::from(token.text_range().end());
                continue;
            }
            last.push(AnnotationToken {
                kind,
                text: token.text().to_owned(),
                end: u32::from(token.text_range().end()),
            });
        }
        let _ = markers;

        let mut lines = Vec::new();
        let mut current = String::new();
        let mut current_level = 0usize;
        let mut open_delimiters: Vec<OpenDelimiter> = Vec::new();
        let mut previous_token: Option<AnnotationTokenKind> = None;
        let mut previous_text = String::new();
        let mut pending_empty_lines = 0usize;
        let mut started = false;

        for tokens in &line_tokens {
            if tokens.is_empty() {
                if started {
                    pending_empty_lines += 1;
                } else {
                    lines.push("#:".to_owned());
                }
                continue;
            }
            for (index, token) in tokens.iter().enumerate() {
                let closer = is_closer(token.kind);
                let closes_expanded = closer
                    && open_delimiters
                        .last()
                        .is_none_or(|delimiter| delimiter.expanded);

                let break_before = if !started {
                    false
                } else if index == 0 && pending_empty_lines > 0 {
                    true
                } else if index == 0 {
                    !closer || closes_expanded
                } else {
                    closes_expanded
                };

                if break_before {
                    lines.push(annotation_line(current_level, &current, &self.indent));
                    current.clear();
                    for _ in 0..pending_empty_lines {
                        lines.push("#:".to_owned());
                    }
                    current_level = match open_delimiters.last() {
                        Some(delimiter) if closer => delimiter.opener_level,
                        Some(delimiter) => delimiter.opener_level + usize::from(delimiter.expanded),
                        None => 0,
                    };
                    previous_token = None;
                    previous_text.clear();
                }
                pending_empty_lines = 0;

                if let Some(previous) = previous_token
                    && annotation_space_between(previous, &previous_text, token.kind)
                {
                    current.push(' ');
                }
                current.push_str(&token.text);
                previous_token = Some(token.kind);
                previous_text = token.text.clone();
                started = true;

                if is_opener(token.kind) {
                    open_delimiters.push(OpenDelimiter {
                        expanded: index + 1 == tokens.len(),
                        opener_level: current_level,
                    });
                } else if closer {
                    open_delimiters.pop();
                }
            }
        }
        if started {
            lines.push(annotation_line(current_level, &current, &self.indent));
        }
        Some(lines)
    }
}

// ---- free helpers ----

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn line_of(line_starts: &[usize], offset: usize) -> usize {
    line_starts
        .partition_point(|&start| start <= offset)
        .saturating_sub(1)
}

fn is_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::PLUS
            | SyntaxKind::MINUS
            | SyntaxKind::STAR
            | SyntaxKind::SLASH
            | SyntaxKind::CARET
            | SyntaxKind::SPECIAL
            | SyntaxKind::PIPE_GREATER
            | SyntaxKind::LESS
            | SyntaxKind::GREATER
            | SyntaxKind::LESS_EQ
            | SyntaxKind::GREATER_EQ
            | SyntaxKind::EQ2
            | SyntaxKind::BANG_EQ
            | SyntaxKind::AMP
            | SyntaxKind::AMP2
            | SyntaxKind::PIPE
            | SyntaxKind::PIPE2
            | SyntaxKind::TILDE
            | SyntaxKind::QUESTION
            | SyntaxKind::LESS_MINUS
            | SyntaxKind::LESS2_MINUS
            | SyntaxKind::MINUS_GREATER
            | SyntaxKind::MINUS_GREATER2
            | SyntaxKind::EQ
            | SyntaxKind::COLON_EQ
            | SyntaxKind::COLON
    )
}

fn parse_directive(text: &str) -> Option<Directive> {
    text.trim_start_matches(|c: char| c.is_whitespace() || c == '#')
        .strip_prefix("fmt:")
        .and_then(|rest| match rest.trim() {
            "skip" => Some(Directive::Skip),
            "skip-file" | "skip file" => Some(Directive::SkipFile),
            "on" => Some(Directive::On),
            "off" => Some(Directive::Off),
            _ => None,
        })
}

// ---- annotation token model ----

struct OpenDelimiter {
    expanded: bool,
    opener_level: usize,
}

struct AnnotationToken {
    kind: AnnotationTokenKind,
    text: String,
    end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationTokenKind {
    Word,
    Directive,
    Dots,
    Arrow,
    Pipe,
    Comma,
    Colon,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    OpenAngle,
    CloseAngle,
}

fn annotation_token_kind(kind: SyntaxKind, text: &str) -> Option<AnnotationTokenKind> {
    use AnnotationTokenKind::*;
    Some(match kind {
        SyntaxKind::L_PAREN => OpenParen,
        SyntaxKind::R_PAREN => CloseParen,
        SyntaxKind::L_BRACKET => OpenBracket,
        SyntaxKind::R_BRACKET => CloseBracket,
        SyntaxKind::L_BRACE => OpenBrace,
        SyntaxKind::R_BRACE => CloseBrace,
        SyntaxKind::LESS => OpenAngle,
        SyntaxKind::GREATER => CloseAngle,
        SyntaxKind::COMMA => Comma,
        SyntaxKind::COLON => Colon,
        SyntaxKind::PIPE => Pipe,
        SyntaxKind::MINUS_GREATER => Arrow,
        SyntaxKind::DOTS => Dots,
        SyntaxKind::AT => Directive,
        SyntaxKind::MINUS => {
            // Interior of a joined directive name (`@if-unknown`); anything
            // else is outside the annotation alphabet.
            return Some(Word);
        }
        _ if kind.is_keyword() => Word,
        SyntaxKind::IDENT | SyntaxKind::DOTDOTI => Word,
        _ => {
            let _ = text;
            return None;
        }
    })
}

fn annotation_line(level: usize, content: &str, indent: &str) -> String {
    format!("#: {}{content}", indent.repeat(level))
}

/// The canonical surface-type spacing: `,` and `:` hug their left neighbor,
/// `|` and `->` are surrounded by spaces, and brackets hug what they apply
/// to.
fn annotation_space_between(
    previous: AnnotationTokenKind,
    previous_text: &str,
    current: AnnotationTokenKind,
) -> bool {
    use AnnotationTokenKind::*;
    match current {
        Comma | Colon | CloseParen | CloseBracket | CloseBrace | CloseAngle => return false,
        OpenParen => return false,
        OpenAngle if previous == Word => return false,
        OpenBracket if matches!(previous, Word | CloseAngle | CloseBracket) => return false,
        OpenBrace if previous == Word && previous_text == "list" => return false,
        Word if previous == Dots => return false,
        _ => {}
    }
    !matches!(previous, OpenParen | OpenBracket | OpenBrace | OpenAngle)
}

fn is_opener(kind: AnnotationTokenKind) -> bool {
    use AnnotationTokenKind::*;
    matches!(kind, OpenParen | OpenBracket | OpenBrace | OpenAngle)
}

fn is_closer(kind: AnnotationTokenKind) -> bool {
    use AnnotationTokenKind::*;
    matches!(kind, CloseParen | CloseBracket | CloseBrace | CloseAngle)
}
