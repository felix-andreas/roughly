//! Hand-written recursive-descent / Pratt parser for R.
//!
//! Grammar shape and precedence follow R's own `gram.y`. Newline significance is
//! tracked with a context stack: inside `(`/`[`/`[[` groups newlines are plain
//! trivia; at the top level and directly inside `{ }` a newline ends a syntactically
//! complete statement. `else` may follow a newline only when some group or brace
//! encloses the `if` (exactly R's rule).
//!
//! Every parse produces a lossless tree: all tokens — including trivia and the
//! tokens of malformed regions — are emitted in order; recovery wraps unparsable
//! stretches in `ERROR` nodes local to the break.

use crate::kind::SyntaxKind;
use crate::lexer::{Token, lex};
use crate::{Parse, SyntaxError};
use rowan::{Checkpoint, GreenNodeBuilder, TextRange, TextSize};

pub(crate) fn parse(text: &str) -> Parse {
    let (tokens, lexer_errors) = lex(text);
    let mut offsets = Vec::with_capacity(tokens.len() + 1);
    let mut offset = 0usize;
    for token in &tokens {
        offsets.push(offset);
        offset += usize::from(token.len);
    }
    offsets.push(offset);

    let mut parser = Parser {
        text,
        tokens,
        offsets,
        pos: 0,
        builder: GreenNodeBuilder::new(),
        errors: lexer_errors,
        groups: vec![0],
        depth: 0,
    };
    parser.source_file();
    let green = parser.builder.finish();
    Parse::new(green, parser.errors)
}

/// Expression nesting beyond this refuses further recursion (with a diagnostic)
/// instead of overflowing the stack on adversarial input.
const MAX_DEPTH: u32 = 500;

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    /// Byte offset of every token (one extra entry: total length).
    offsets: Vec<usize>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
    /// Newline-significance stack: entering `{` pushes 0, `(`/`[`/`[[` increments
    /// the top, closers undo. Newlines are significant iff the top is 0.
    groups: Vec<u32>,
    depth: u32,
}

impl Parser<'_> {
    // ---- token access ----

    fn current(&self) -> Option<SyntaxKind> {
        self.tokens.get(self.pos).map(|token| token.kind)
    }

    fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == Some(kind)
    }

    fn token_text(&self, pos: usize) -> &str {
        &self.text[self.offsets[pos]..self.offsets[pos + 1]]
    }

    fn token_range(&self, pos: usize) -> TextRange {
        let (start, end) = if pos < self.tokens.len() {
            (self.offsets[pos], self.offsets[pos + 1])
        } else {
            (self.text.len(), self.text.len())
        };
        TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
    }

    /// The next significant token at or after `pos`, looking through trivia.
    /// `across_newlines` controls whether newlines are looked through.
    fn peek_significant(&self, mut pos: usize, across_newlines: bool) -> Option<(SyntaxKind, usize)> {
        loop {
            let token = self.tokens.get(pos)?;
            match token.kind {
                SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => pos += 1,
                SyntaxKind::NEWLINE if across_newlines => pos += 1,
                // An annotation region is trivia for grammar purposes.
                SyntaxKind::ANNOTATION_MARKER => pos = self.annotation_region_end(pos),
                kind => return Some((kind, pos)),
            }
        }
    }

    /// Token index just past the stitched annotation region whose marker is at `pos`.
    fn annotation_region_end(&self, mut pos: usize) -> usize {
        debug_assert_eq!(self.tokens[pos].kind, SyntaxKind::ANNOTATION_MARKER);
        loop {
            // Consume the marker's line.
            while let Some(token) = self.tokens.get(pos) {
                if token.kind == SyntaxKind::NEWLINE {
                    break;
                }
                pos += 1;
            }
            // Stitch when the next line (past one newline and indentation) is `#:` again.
            let Some(token) = self.tokens.get(pos) else { return pos };
            debug_assert_eq!(token.kind, SyntaxKind::NEWLINE);
            let mut lookahead = pos + 1;
            while self.tokens.get(lookahead).is_some_and(|t| t.kind == SyntaxKind::WHITESPACE) {
                lookahead += 1;
            }
            if self.tokens.get(lookahead).is_some_and(|t| t.kind == SyntaxKind::ANNOTATION_MARKER) {
                pos = lookahead + 1;
            } else {
                return pos;
            }
        }
    }

    fn newlines_significant(&self) -> bool {
        *self.groups.last().expect("group stack is never empty") == 0
    }

    fn else_allowed_across_newline(&self) -> bool {
        !self.newlines_significant() || self.groups.len() > 1
    }

    // ---- tree building ----

    fn bump(&mut self) {
        let kind = self.current().expect("bump at end of input");
        let text = &self.text[self.offsets[self.pos]..self.offsets[self.pos + 1]];
        self.builder.token(kind.into(), text);
        self.pos += 1;
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(kind.into());
    }

    fn start_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder.start_node_at(checkpoint, kind.into());
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }

    fn checkpoint(&mut self) -> Checkpoint {
        self.builder.checkpoint()
    }

    fn wrap_name(&mut self) {
        self.start(SyntaxKind::NAME);
        self.bump();
        self.finish();
    }

    fn wrap_literal(&mut self) {
        self.start(SyntaxKind::LITERAL);
        self.bump();
        self.finish();
    }

    // ---- errors ----

    fn error_here(&mut self, message: impl Into<String>) {
        let range = self.token_range(self.pos);
        self.errors.push(SyntaxError::new(message, range));
    }

    fn error_at(&mut self, range: TextRange, message: impl Into<String>) {
        self.errors.push(SyntaxError::new(message, range));
    }

    /// Describe the current token for an "expected …, found …" message.
    fn describe_current(&self) -> String {
        match self.current() {
            None => "end of file".to_owned(),
            Some(SyntaxKind::NEWLINE) => "end of line".to_owned(),
            Some(kind) => kind.display().to_owned(),
        }
    }

    // ---- trivia ----

    /// Emit trivia into the tree. Newlines are consumed only when insignificant
    /// in the current context or explicitly allowed by `across_newlines`.
    fn eat_trivia(&mut self, across_newlines: bool) {
        loop {
            match self.current() {
                Some(SyntaxKind::WHITESPACE | SyntaxKind::COMMENT) => self.bump(),
                Some(SyntaxKind::NEWLINE) if across_newlines || !self.newlines_significant() => {
                    self.bump()
                }
                Some(SyntaxKind::ANNOTATION_MARKER) => self.annotation(),
                _ => return,
            }
        }
    }

    /// One stitched `#:` annotation region as a first-class node. The annotation
    /// body grammar (types, directives) is parsed by a later slice; for now the
    /// body tokens are preserved verbatim inside the node.
    fn annotation(&mut self) {
        self.start(SyntaxKind::ANNOTATION);
        let end = self.annotation_region_end(self.pos);
        while self.pos < end {
            self.bump();
        }
        self.finish();
    }

    // ---- statements ----

    fn source_file(&mut self) {
        self.start(SyntaxKind::SOURCE_FILE);
        self.statements(None);
        // Defensive: anything the statement loop could not consume.
        if self.pos < self.tokens.len() {
            self.start(SyntaxKind::ERROR);
            while self.pos < self.tokens.len() {
                self.bump();
            }
            self.finish();
        }
        self.finish();
    }

    /// Statement sequence until `terminator` (or end of file). Used for the
    /// source file (`None`) and brace bodies (`Some(R_BRACE)`).
    fn statements(&mut self, terminator: Option<SyntaxKind>) {
        loop {
            self.eat_trivia(true);
            match self.current() {
                Some(SyntaxKind::SEMICOLON) => {
                    self.bump();
                    continue;
                }
                None => return,
                Some(kind) if Some(kind) == terminator => return,
                _ => {}
            }

            if !self.expression(0) {
                match self.current() {
                    Some(SyntaxKind::ELSE_KW) => self.error_here(
                        "unexpected `else`: it must stay on the same line as the `if` branch it belongs to (or the `if` must be inside braces)",
                    ),
                    Some(SyntaxKind::R_BRACE) if terminator.is_none() => {
                        self.error_here("unmatched `}`: no `{` is open here")
                    }
                    _ => {
                        let description = self.describe_current();
                        self.error_here(format!("expected a statement, found {description}"));
                    }
                }
                // Consume through the end of the line so one broken construct
                // stays local.
                self.start(SyntaxKind::ERROR);
                self.recover_to_statement_boundary(terminator);
                self.finish();
                continue;
            }

            // After a complete statement: a newline, `;`, the terminator, or EOF.
            self.eat_trivia(false);
            match self.current() {
                None | Some(SyntaxKind::NEWLINE | SyntaxKind::SEMICOLON) => {}
                Some(kind) if Some(kind) == terminator => {}
                Some(_) => {
                    let description = self.describe_current();
                    self.error_here(format!(
                        "unexpected {description} after this expression; expected a newline or `;`"
                    ));
                    self.start(SyntaxKind::ERROR);
                    self.recover_to_statement_boundary(terminator);
                    self.finish();
                }
            }
        }
    }

    fn recover_to_statement_boundary(&mut self, terminator: Option<SyntaxKind>) {
        while let Some(kind) = self.current() {
            if kind == SyntaxKind::NEWLINE || kind == SyntaxKind::SEMICOLON || Some(kind) == terminator {
                return;
            }
            self.bump();
        }
    }

    // ---- expressions ----

    /// Pratt loop. Returns false — emitting nothing and consuming nothing — when
    /// the current token cannot start an expression; callers report the
    /// context-specific error.
    fn expression(&mut self, min_bp: u8) -> bool {
        if self.depth >= MAX_DEPTH {
            self.error_here("expression nesting is too deep");
            return false;
        }
        self.depth += 1;
        let produced = self.expression_inner(min_bp);
        self.depth -= 1;
        produced
    }

    fn expression_inner(&mut self, min_bp: u8) -> bool {
        let checkpoint = self.checkpoint();
        if !self.primary() {
            return false;
        }

        loop {
            self.eat_trivia(false);
            let Some(kind) = self.current() else { break };
            match kind {
                // Postfix: calls and subscripts (highest precedence).
                SyntaxKind::L_PAREN if CALL_BP >= min_bp => {
                    self.start_at(checkpoint, SyntaxKind::CALL_EXPR);
                    self.argument_list(SyntaxKind::L_PAREN);
                    self.finish();
                }
                SyntaxKind::L_BRACKET if CALL_BP >= min_bp => {
                    self.start_at(checkpoint, SyntaxKind::SUBSET_EXPR);
                    self.argument_list(SyntaxKind::L_BRACKET);
                    self.finish();
                }
                SyntaxKind::L_BRACKET2 if CALL_BP >= min_bp => {
                    self.start_at(checkpoint, SyntaxKind::SUBSET2_EXPR);
                    self.argument_list(SyntaxKind::L_BRACKET2);
                    self.finish();
                }
                // `$` / `@` and `::` / `:::` take a name (or string) on the right.
                SyntaxKind::DOLLAR | SyntaxKind::AT if FIELD_BP >= min_bp => {
                    let node = if kind == SyntaxKind::DOLLAR {
                        SyntaxKind::DOLLAR_EXPR
                    } else {
                        SyntaxKind::AT_EXPR
                    };
                    self.start_at(checkpoint, node);
                    self.bump();
                    self.eat_trivia(true);
                    self.field_name(kind);
                    self.finish();
                }
                SyntaxKind::COLON2 | SyntaxKind::COLON3 if NAMESPACE_BP >= min_bp => {
                    self.start_at(checkpoint, SyntaxKind::NAMESPACE_EXPR);
                    self.bump();
                    self.eat_trivia(true);
                    self.field_name(kind);
                    self.finish();
                }
                _ => {
                    let Some((left_bp, right_bp)) = infix_binding_power(kind) else { break };
                    if left_bp < min_bp {
                        break;
                    }
                    self.start_at(checkpoint, SyntaxKind::BINARY_EXPR);
                    let operator_range = self.token_range(self.pos);
                    let operator = self.describe_current();
                    self.bump();
                    self.eat_trivia(true);
                    if !self.expression(right_bp) {
                        self.error_at(operator_range, format!("expected an expression after {operator}"));
                    }
                    self.finish();
                }
            }
        }
        true
    }

    /// A name or string after `$`, `@`, `::`, `:::`.
    fn field_name(&mut self, operator: SyntaxKind) {
        match self.current() {
            Some(SyntaxKind::IDENT | SyntaxKind::DOTS | SyntaxKind::DOTDOTI) => self.wrap_name(),
            Some(SyntaxKind::STRING) => self.wrap_literal(),
            _ => {
                let description = self.describe_current();
                self.error_here(format!(
                    "expected a name after {}, found {description}",
                    operator.display()
                ));
            }
        }
    }

    /// Prefix operators and primary expressions. Quiet on failure: consumes and
    /// emits nothing, returns false.
    fn primary(&mut self) -> bool {
        let Some(kind) = self.current() else { return false };
        match kind {
            SyntaxKind::IDENT | SyntaxKind::DOTS | SyntaxKind::DOTDOTI | SyntaxKind::UNDERSCORE => {
                self.wrap_name()
            }
            SyntaxKind::INTEGER
            | SyntaxKind::DOUBLE
            | SyntaxKind::COMPLEX
            | SyntaxKind::STRING
            | SyntaxKind::RAW_STRING
            | SyntaxKind::TRUE_KW
            | SyntaxKind::FALSE_KW
            | SyntaxKind::NULL_KW
            | SyntaxKind::INF_KW
            | SyntaxKind::NAN_KW
            | SyntaxKind::NA_KW
            | SyntaxKind::NA_INTEGER_KW
            | SyntaxKind::NA_REAL_KW
            | SyntaxKind::NA_COMPLEX_KW
            | SyntaxKind::NA_CHARACTER_KW => self.wrap_literal(),
            SyntaxKind::MINUS | SyntaxKind::PLUS => self.unary(UNARY_SIGN_BP),
            SyntaxKind::BANG => self.unary(UNARY_NOT_BP),
            SyntaxKind::TILDE => self.unary(TILDE_BP + 1),
            SyntaxKind::QUESTION => self.unary(HELP_BP + 1),
            SyntaxKind::L_PAREN => {
                self.start(SyntaxKind::PAREN_EXPR);
                let open_range = self.token_range(self.pos);
                self.open_group();
                self.bump();
                self.eat_trivia(true);
                if !self.expression(0) {
                    let description = self.describe_current();
                    self.error_here(format!("expected an expression inside `(`, found {description}"));
                }
                self.eat_trivia(true);
                self.end_group();
                if self.at(SyntaxKind::R_PAREN) {
                    self.bump();
                } else {
                    self.error_at(open_range, "unclosed `(`; expected a matching `)`");
                }
                self.finish();
            }
            SyntaxKind::L_BRACE => {
                self.start(SyntaxKind::BRACE_EXPR);
                let open_range = self.token_range(self.pos);
                self.groups.push(0);
                self.bump();
                self.statements(Some(SyntaxKind::R_BRACE));
                self.groups.pop();
                if self.at(SyntaxKind::R_BRACE) {
                    self.bump();
                } else {
                    self.error_at(open_range, "unclosed `{`; expected a matching `}`");
                }
                self.finish();
            }
            SyntaxKind::IF_KW => self.if_expr(),
            SyntaxKind::FOR_KW => self.for_expr(),
            SyntaxKind::WHILE_KW => self.while_expr(),
            SyntaxKind::REPEAT_KW => {
                self.start(SyntaxKind::REPEAT_EXPR);
                self.bump();
                self.eat_trivia(true);
                if !self.expression(0) {
                    self.error_here("expected a body after `repeat`");
                }
                self.finish();
            }
            SyntaxKind::FUNCTION_KW | SyntaxKind::BACKSLASH => self.function_def(),
            SyntaxKind::BREAK_KW => {
                self.start(SyntaxKind::BREAK_EXPR);
                self.bump();
                self.finish();
            }
            SyntaxKind::NEXT_KW => {
                self.start(SyntaxKind::NEXT_EXPR);
                self.bump();
                self.finish();
            }
            _ => return false,
        }
        true
    }

    fn unary(&mut self, right_bp: u8) {
        self.start(SyntaxKind::UNARY_EXPR);
        let operator = self.describe_current();
        let operator_range = self.token_range(self.pos);
        self.bump();
        self.eat_trivia(true);
        if !self.expression(right_bp) {
            self.error_at(operator_range, format!("expected an expression after {operator}"));
        }
        self.finish();
    }

    fn if_expr(&mut self) {
        self.start(SyntaxKind::IF_EXPR);
        self.bump();
        self.eat_trivia(true);
        self.condition_parens("if");
        self.eat_trivia(true);
        if !self.expression(0) {
            let description = self.describe_current();
            self.error_here(format!("expected a branch after the `if` condition, found {description}"));
        }
        // `else` may follow a newline only inside a group or brace context.
        let across = self.else_allowed_across_newline();
        if let Some((SyntaxKind::ELSE_KW, _)) = self.peek_significant(self.pos, across) {
            self.eat_trivia(across);
            debug_assert!(self.at(SyntaxKind::ELSE_KW));
            self.bump();
            self.eat_trivia(true);
            if !self.expression(0) {
                let description = self.describe_current();
                self.error_here(format!("expected an expression after `else`, found {description}"));
            }
        }
        self.finish();
    }

    fn for_expr(&mut self) {
        self.start(SyntaxKind::FOR_EXPR);
        self.bump();
        self.eat_trivia(true);
        if self.at(SyntaxKind::L_PAREN) {
            let open_range = self.token_range(self.pos);
            self.open_group();
            self.bump();
            self.eat_trivia(true);
            if self.at(SyntaxKind::IDENT) {
                self.wrap_name();
            } else {
                let description = self.describe_current();
                self.error_here(format!("expected a loop variable after `for (`, found {description}"));
            }
            self.eat_trivia(true);
            if self.at(SyntaxKind::IN_KW) {
                self.bump();
            } else {
                let description = self.describe_current();
                self.error_here(format!("expected `in` in the `for` head, found {description}"));
            }
            self.eat_trivia(true);
            if !self.expression(0) {
                let description = self.describe_current();
                self.error_here(format!("expected a sequence expression in the `for` head, found {description}"));
            }
            self.eat_trivia(true);
            self.end_group();
            if self.at(SyntaxKind::R_PAREN) {
                self.bump();
            } else {
                self.error_at(open_range, "unclosed `(` in the `for` head; expected a matching `)`");
            }
        } else {
            let description = self.describe_current();
            self.error_here(format!("expected `(` after `for`, found {description}"));
        }
        self.eat_trivia(true);
        if !self.expression(0) {
            self.error_here("expected a body for the `for` loop");
        }
        self.finish();
    }

    fn while_expr(&mut self) {
        self.start(SyntaxKind::WHILE_EXPR);
        self.bump();
        self.eat_trivia(true);
        self.condition_parens("while");
        self.eat_trivia(true);
        if !self.expression(0) {
            self.error_here("expected a body for the `while` loop");
        }
        self.finish();
    }

    /// `( expr )` after `if` / `while`.
    fn condition_parens(&mut self, construct: &str) {
        if !self.at(SyntaxKind::L_PAREN) {
            let description = self.describe_current();
            self.error_here(format!("expected `(` after `{construct}`, found {description}"));
            return;
        }
        let open_range = self.token_range(self.pos);
        self.open_group();
        self.bump();
        self.eat_trivia(true);
        if !self.expression(0) {
            let description = self.describe_current();
            self.error_here(format!("expected a condition inside `{construct} (…)`, found {description}"));
        }
        self.eat_trivia(true);
        self.end_group();
        if self.at(SyntaxKind::R_PAREN) {
            self.bump();
        } else {
            self.error_at(
                open_range,
                format!("unclosed `(` in the `{construct}` condition; expected a matching `)`"),
            );
        }
    }

    fn function_def(&mut self) {
        self.start(SyntaxKind::FUNCTION_DEF);
        self.bump();
        self.eat_trivia(true);
        if self.at(SyntaxKind::L_PAREN) {
            self.parameter_list();
        } else {
            let description = self.describe_current();
            self.error_here(format!("expected `(` to open the parameter list, found {description}"));
        }
        self.eat_trivia(true);
        if !self.expression(0) {
            self.error_here("expected a function body");
        }
        self.finish();
    }

    fn parameter_list(&mut self) {
        self.start(SyntaxKind::PARAMETER_LIST);
        let open_range = self.token_range(self.pos);
        self.open_group();
        self.bump();
        loop {
            self.eat_trivia(true);
            match self.current() {
                None | Some(SyntaxKind::R_PAREN) => break,
                Some(SyntaxKind::IDENT | SyntaxKind::DOTS | SyntaxKind::DOTDOTI) => {
                    self.start(SyntaxKind::PARAMETER);
                    self.bump();
                    self.eat_trivia(true);
                    if self.at(SyntaxKind::EQ) {
                        self.bump();
                        self.eat_trivia(true);
                        match self.current() {
                            Some(SyntaxKind::COMMA | SyntaxKind::R_PAREN) | None => {
                                self.error_here("expected a default value after `=`");
                            }
                            _ => {
                                if !self.expression(0) {
                                    self.error_here("expected a default value after `=`");
                                }
                            }
                        }
                    }
                    self.finish();
                    self.eat_trivia(true);
                    if self.at(SyntaxKind::COMMA) {
                        self.bump();
                    } else if !matches!(self.current(), None | Some(SyntaxKind::R_PAREN)) {
                        let description = self.describe_current();
                        self.error_here(format!(
                            "expected `,` or `)` in the parameter list, found {description}"
                        ));
                        self.recover_argument(SyntaxKind::R_PAREN);
                        if self.at(SyntaxKind::COMMA) {
                            self.bump();
                        }
                    }
                }
                _ => {
                    let description = self.describe_current();
                    self.error_here(format!("expected a parameter name, found {description}"));
                    self.recover_argument(SyntaxKind::R_PAREN);
                    if self.at(SyntaxKind::COMMA) {
                        self.bump();
                    }
                }
            }
        }
        self.end_group();
        if self.at(SyntaxKind::R_PAREN) {
            self.bump();
        } else {
            self.error_at(open_range, "unclosed `(`; expected `)` to close the parameter list");
        }
        self.finish();
    }

    /// Call / subset argument lists. `open` is `(`, `[`, or `[[`; `[[` closes
    /// with two `]` tokens (as in R's grammar).
    fn argument_list(&mut self, open: SyntaxKind) {
        self.start(SyntaxKind::ARGUMENT_LIST);
        let open_range = self.token_range(self.pos);
        self.open_group();
        self.bump();
        let closer = match open {
            SyntaxKind::L_PAREN => SyntaxKind::R_PAREN,
            _ => SyntaxKind::R_BRACKET,
        };
        let mut expect_argument = true;
        loop {
            self.eat_trivia(true);
            match self.current() {
                None => break,
                Some(kind) if kind == closer => break,
                Some(SyntaxKind::COMMA) => {
                    if expect_argument {
                        // A positional hole: `f(, x)` / `m[, 1]`.
                        self.start(SyntaxKind::ARGUMENT);
                        self.finish();
                    }
                    self.bump();
                    expect_argument = true;
                }
                Some(_) => {
                    if !expect_argument {
                        let description = self.describe_current();
                        self.error_here(format!(
                            "expected `,` or {} in this argument list, found {description}",
                            closer.display()
                        ));
                        self.recover_argument(closer);
                        continue;
                    }
                    self.argument(closer);
                    expect_argument = false;
                }
            }
        }
        self.end_group();
        if self.at(closer) {
            self.bump();
            if open == SyntaxKind::L_BRACKET2 {
                // The second `]` of `]]`.
                self.eat_trivia(true);
                if self.at(SyntaxKind::R_BRACKET) {
                    self.bump();
                } else {
                    self.error_at(open_range, "expected `]]` to close `[[`");
                }
            }
        } else {
            self.error_at(
                open_range,
                format!("unclosed {}; expected {} to close it", open.display(), closer.display()),
            );
        }
        self.finish();
    }

    fn argument(&mut self, closer: SyntaxKind) {
        self.start(SyntaxKind::ARGUMENT);
        // Tagged argument: `name = value`, `"name" = value`, `... = value`.
        let tagged = matches!(
            self.current(),
            Some(SyntaxKind::IDENT | SyntaxKind::STRING | SyntaxKind::DOTS | SyntaxKind::DOTDOTI)
        ) && self
            .peek_significant(self.pos + 1, true)
            .is_some_and(|(kind, _)| kind == SyntaxKind::EQ);
        if tagged {
            match self.current() {
                Some(SyntaxKind::STRING) => self.wrap_literal(),
                _ => self.wrap_name(),
            }
            self.eat_trivia(true);
            debug_assert!(self.at(SyntaxKind::EQ));
            self.bump();
            self.eat_trivia(true);
            match self.current() {
                // `f(x = )` and `f(x = , y)` leave the value empty.
                Some(SyntaxKind::COMMA) | None => {}
                Some(kind) if kind == closer => {}
                _ => {
                    if !self.expression(0) {
                        let description = self.describe_current();
                        self.error_here(format!("expected an argument value, found {description}"));
                        self.recover_argument(closer);
                    }
                }
            }
        } else if !self.expression(0) {
            let description = self.describe_current();
            self.error_here(format!("expected an argument, found {description}"));
            self.recover_argument(closer);
        }
        self.finish();
    }

    /// Recovery inside a delimited list, wrapped in an `ERROR` node and
    /// guaranteed to make progress: a mismatched closing delimiter sitting at
    /// the recovery point is consumed (it can belong to nothing here), so the
    /// surrounding list loop can never spin in place.
    fn recover_argument(&mut self, closer: SyntaxKind) {
        let start = self.pos;
        self.start(SyntaxKind::ERROR);
        self.recover_in_list(closer);
        if self.pos == start
            && !matches!(self.current(), None | Some(SyntaxKind::COMMA))
            && self.current() != Some(closer)
        {
            self.bump();
            self.recover_in_list(closer);
        }
        self.finish();
    }

    /// Consume tokens until `,`, the closing delimiter, or end of file, keeping
    /// broken argument regions local.
    fn recover_in_list(&mut self, closer: SyntaxKind) {
        let mut depth = 0u32;
        while let Some(kind) = self.current() {
            match kind {
                SyntaxKind::COMMA if depth == 0 => return,
                kind if kind == closer && depth == 0 => return,
                SyntaxKind::L_PAREN | SyntaxKind::L_BRACKET | SyntaxKind::L_BRACKET2 | SyntaxKind::L_BRACE => {
                    depth += 1;
                    self.bump();
                }
                SyntaxKind::R_PAREN | SyntaxKind::R_BRACKET | SyntaxKind::R_BRACE => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.bump();
                }
                _ => self.bump(),
            }
        }
    }

    fn open_group(&mut self) {
        if let Some(depth) = self.groups.last_mut() {
            *depth += 1;
        }
    }

    fn end_group(&mut self) {
        if let Some(depth) = self.groups.last_mut() {
            *depth = depth.saturating_sub(1);
        }
    }
}

// Binding powers follow R's `gram.y` precedence declarations, low to high.
// Left-associative operators parse their right side at `left + 1`; right-
// associative ones at `left`.

const CALL_BP: u8 = 40;
const NAMESPACE_BP: u8 = 38;
const FIELD_BP: u8 = 36;
const POW_BP: u8 = 34;
const UNARY_SIGN_BP: u8 = 32;
const COLON_BP: u8 = 30;
const SPECIAL_BP: u8 = 28;
const MUL_BP: u8 = 26;
const ADD_BP: u8 = 24;
const COMPARE_BP: u8 = 22;
const UNARY_NOT_BP: u8 = 20;
const AND_BP: u8 = 18;
const OR_BP: u8 = 16;
const TILDE_BP: u8 = 14;
const RIGHT_ASSIGN_BP: u8 = 12;
const EQ_ASSIGN_BP: u8 = 10;
const LEFT_ASSIGN_BP: u8 = 8;
const HELP_BP: u8 = 2;

fn infix_binding_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    use SyntaxKind::*;
    let (left, right) = match kind {
        QUESTION => (HELP_BP, HELP_BP + 1),
        LESS_MINUS | LESS2_MINUS | COLON_EQ => (LEFT_ASSIGN_BP, LEFT_ASSIGN_BP),
        EQ => (EQ_ASSIGN_BP, EQ_ASSIGN_BP),
        MINUS_GREATER | MINUS_GREATER2 => (RIGHT_ASSIGN_BP, RIGHT_ASSIGN_BP + 1),
        TILDE => (TILDE_BP, TILDE_BP + 1),
        PIPE2 | PIPE => (OR_BP, OR_BP + 1),
        AMP2 | AMP => (AND_BP, AND_BP + 1),
        EQ2 | BANG_EQ | LESS | GREATER | LESS_EQ | GREATER_EQ => (COMPARE_BP, COMPARE_BP + 1),
        PLUS | MINUS => (ADD_BP, ADD_BP + 1),
        STAR | SLASH => (MUL_BP, MUL_BP + 1),
        SPECIAL | PIPE_GREATER => (SPECIAL_BP, SPECIAL_BP + 1),
        COLON => (COLON_BP, COLON_BP + 1),
        CARET => (POW_BP, POW_BP - 1),
        _ => return None,
    };
    Some((left, right))
}
