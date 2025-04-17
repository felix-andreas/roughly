use {
    nu_ansi_term::{Color as AnsiColor, Style},
    reedline::{
        Color, DefaultHinter, Emacs, Highlighter, Prompt, PromptEditMode, PromptHistorySearch,
        PromptHistorySearchStatus, PromptViMode, Reedline, Signal, StyledText, Vi,
    },
    std::{
        borrow::Cow,
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    },
};

pub fn run(vi: bool) {
    let highligher = RoughlyHighlighter;
    let hinter = DefaultHinter::default().with_style(Style::new().fg(AnsiColor::DarkGray));
    let prompt = RoughlyPrompt;

    let mut line_editor = Reedline::create()
        .with_highlighter(Box::new(highligher))
        .with_hinter(Box::new(hinter))
        .with_edit_mode(if vi {
            Box::new(Vi::default())
        } else {
            Box::new(Emacs::default())
        });

    let mut r_process = match Command::new("R")
        .arg("--no-readline")
        .arg("--no-save")
        .arg("--quiet")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Failed to start R process: {}", e);
            return;
        }
    };
    let Some(stdout) = r_process.stdout.as_mut() else {
        eprintln!("Failed to get R process stdout.");
        return;
    };
    let mut reader = BufReader::new(stdout);
    let mut line = String::with_capacity(256);

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if buffer.is_empty() {
                    continue;
                }
                if let Some(stdin) = r_process.stdin.as_mut() {
                    if let Err(e) = writeln!(stdin, "{}", buffer) {
                        eprintln!("Failed to write to R process stdin: {}", e);
                        break;
                    }
                } else {
                    eprintln!("R process stdin is not available.");
                    break;
                }

                loop {
                    line.clear();
                    let _bytes = reader.read_line(&mut line).unwrap_or(0);
                    // print!("{}", line);

                    // Read until R prompt appears (">> " at line start)
                    if line.trim_start().starts_with(">>") {
                        break;
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

struct RoughlyHighlighter;

impl Highlighter for RoughlyHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled_text = StyledText::new();
        styled_text.push((Style::new().fg(AnsiColor::Default), line.to_string()));
        styled_text
    }
}
