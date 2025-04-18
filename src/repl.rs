use {
    extendr_api::{Rinternals, eval_string},
    nu_ansi_term::{Color as AnsiColor, Style},
    reedline::{
        Color, DefaultHinter, Emacs, Highlighter, Prompt, PromptEditMode, PromptHistorySearch,
        PromptHistorySearchStatus, PromptViMode, Reedline, Signal, StyledText, ValidationResult,
        Validator, Vi,
    },
    std::{
        borrow::Cow,
        ops::Range,
        sync::{Arc, RwLock},
    },
    tree_sitter::{Parser, TreeCursor},
};

pub fn run(vi: bool) {
    let language = Arc::new(RwLock::new(Language::new()));

    let highligher = RoughlyHighlighter::new(Arc::clone(&language));
    let hinter = DefaultHinter::default().with_style(Style::new().fg(AnsiColor::DarkGray));
    let validator = RoughlyValidator::new(Arc::clone(&language));
    let prompt = RoughlyPrompt;

    let mut line_editor = Reedline::create()
        .with_highlighter(Box::new(highligher))
        .with_hinter(Box::new(hinter))
        .with_validator(Box::new(validator))
        .with_edit_mode(if vi {
            Box::new(Vi::default())
        } else {
            Box::new(Emacs::default())
        });

    extendr_engine::start_r();

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if buffer.is_empty() {
                    continue;
                }

                if let Ok(value) = eval_string(&buffer) {
                    if !value.is_null() {
                        println!("{:?}", value);
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                continue;
            }
            Ok(Signal::CtrlD) => {
                break;
            }
            Err(error) => {
                println!("error: {:?}", error);
            }
        }
    }
}

struct RoughlyPrompt;

impl Prompt for RoughlyPrompt {
    fn render_prompt_left(&self) -> Cow<str> {
        "".into()
    }

    fn render_prompt_right(&self) -> Cow<str> {
        "".into()
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => "> ".into(),
            PromptEditMode::Vi(vi_mode) => match vi_mode {
                PromptViMode::Normal => "! ".into(),
                PromptViMode::Insert => "> ".into(),
            },
            PromptEditMode::Custom(str) => format!("({str})").into(),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<str> {
        "| ".into()
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };

        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }

    fn get_prompt_color(&self) -> Color {
        Color::Reset
    }

    fn get_prompt_multiline_color(&self) -> AnsiColor {
        AnsiColor::Default
    }

    fn get_indicator_color(&self) -> Color {
        Color::Reset
    }

    fn get_prompt_right_color(&self) -> Color {
        Color::Reset
    }

    fn right_prompt_on_last_line(&self) -> bool {
        false
    }
}

struct RoughlyHighlighter {
    language: Arc<RwLock<Language>>,
}

impl RoughlyHighlighter {
    fn new(language: Arc<RwLock<Language>>) -> Self {
        Self { language }
    }
}

impl Highlighter for RoughlyHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled_text = StyledText::new();

        let tree = {
            let mut language = self.language.write().unwrap();
            language.parser.parse(line, None).unwrap()
        };

        let mut styled_ranges = Vec::new();
        traverse(&mut tree.root_node().walk(), &mut styled_ranges);

        let mut last_end = 0;
        for (style, range) in styled_ranges {
            if last_end < range.start {
                styled_text.push((
                    Style::new().fg(AnsiColor::DarkGray),
                    line[last_end..range.start].to_string(),
                ));
            }

            styled_text.push((style, line[range.clone()].to_string()));
            last_end = range.end;
        }

        if last_end < line.len() {
            styled_text.push((
                Style::new().fg(AnsiColor::DarkGray),
                line[last_end..].to_string(),
            ));
        }

        styled_text
    }
}

fn traverse(cursor: &mut TreeCursor, state: &mut Vec<(Style, Range<usize>)>) {
    let node = cursor.node();

    if node.has_error() {
        let style = Style::new().underline().fg(AnsiColor::LightRed);
        state.push((style, node.byte_range()));
        return;
    }

    match node.kind() {
        "identifier" => {
            let style = Style::new().fg(AnsiColor::Default);
            state.push((style, node.byte_range()));
        }
        "string" => {
            let style = Style::new().fg(AnsiColor::Green);
            state.push((style, node.byte_range()));
        }
        "integer" | "float" | "complex" => {
            let style = Style::new().fg(AnsiColor::Blue);
            state.push((style, node.byte_range()));
        }
        _ => {}
    }

    if cursor.goto_first_child() {
        loop {
            traverse(cursor, state);

            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
        }
    }
}

struct Language {
    parser: Parser,
    // TODO: use this to store the tree
    // tree: Option<Tree>,
}

impl Language {
    fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .unwrap();
        Self { parser }
    }
}

struct RoughlyValidator {
    language: Arc<RwLock<Language>>,
}

impl RoughlyValidator {
    fn new(language: Arc<RwLock<Language>>) -> Self {
        Self { language }
    }
}

impl Validator for RoughlyValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let tree = {
            let mut language = self.language.write().unwrap();
            language.parser.parse(line, None).unwrap()
        };
        let node = tree.root_node();

        if node.has_error() {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Complete
        }
    }
}
