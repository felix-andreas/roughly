//! Literate R documents: R Markdown (`.Rmd`), Quarto (`.qmd`) and Sweave
//! (`.Rnw`).
//!
//! Analysis reads one of these as the R program it contains, by blanking
//! everything that is not R: every prose byte becomes a space and every newline
//! stays a newline. That keeps the converted text **byte-for-byte the same
//! length** as the original, so every range a diagnostic reports — and every
//! position the editor sends — needs no translation at either end. It also makes
//! the whole document one R script, which is what it is at knit time: a chunk
//! sees the bindings earlier chunks created.

/// Whether a file extension names a literate R document.
pub fn is_literate_extension(extension: &str) -> bool {
    matches!(
        extension,
        "Rmd" | "rmd" | "qmd" | "Rnw" | "rnw" | "Snw" | "snw"
    )
}

/// The R program a literate document contains, with prose blanked to spaces so
/// offsets match the original text exactly.
pub fn r_source_of_literate(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_chunk = false;
    for line in split_keeping_terminators(text) {
        let body = line.trim_end_matches(['\n', '\r']);
        let terminator = &line[body.len()..];
        let trimmed = body.trim_start();
        let keep = if in_chunk {
            if is_chunk_end(trimmed) {
                in_chunk = false;
                false
            } else {
                // A chunk option line (`#| echo: false`) is a comment in R, so
                // it survives conversion untouched and needs no special case.
                true
            }
        } else {
            if is_chunk_start(trimmed) {
                in_chunk = true;
            }
            false
        };
        if keep {
            out.push_str(body);
        } else {
            out.extend(body.chars().map(|character| {
                // Blank per CHARACTER, not per byte: pushing one space for a
                // multi-byte character would shorten the text and shift every
                // range after it. A space per character keeps char offsets
                // aligned, and the byte length too whenever the source is
                // ASCII — the case where a range could otherwise land inside a
                // character.
                if character.is_whitespace() {
                    character
                } else {
                    ' '
                }
            }));
        }
        out.push_str(terminator);
    }
    out
}

/// A fenced R chunk's opening line: ```` ```{r ... } ```` (Markdown) or
/// `<<...>>=` (Sweave). A fence naming another language is prose.
fn is_chunk_start(trimmed: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix("<<") {
        return rest.ends_with(">>=");
    }
    let Some(rest) = fence_body(trimmed) else {
        return false;
    };
    let Some(inside) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) else {
        return false;
    };
    let language = inside
        .split([',', ' ', '\t'])
        .next()
        .unwrap_or_default()
        .trim();
    language.eq_ignore_ascii_case("r")
}

/// A chunk's closing line: a bare fence, or Sweave's `@`.
fn is_chunk_end(trimmed: &str) -> bool {
    trimmed == "@" || fence_body(trimmed).is_some_and(str::is_empty)
}

/// The text after a Markdown fence's backticks, when the line opens with at
/// least three of them.
fn fence_body(trimmed: &str) -> Option<&str> {
    let backticks = trimmed.chars().take_while(|&c| c == '`').count();
    (backticks >= 3).then(|| trimmed[backticks..].trim())
}

/// Lines with their line terminators attached, so the conversion can reproduce
/// `\n` and `\r\n` exactly rather than normalizing them.
fn split_keeping_terminators(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.find('\n').map_or(rest.len(), |index| index + 1);
        let (line, remainder) = rest.split_at(end);
        rest = remainder;
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_preserves_length_and_keeps_chunk_code() {
        let document = "# Title\n\nSome prose.\n\n```{r setup}\nx <- 1L\n```\n\nMore prose.\n\n```{r}\nprint(x)\n```\n";
        let converted = r_source_of_literate(document);
        assert_eq!(converted.len(), document.len(), "{converted:?}");
        assert!(converted.contains("x <- 1L"));
        assert!(converted.contains("print(x)"));
        assert!(!converted.contains("Title"));
        assert!(!converted.contains("prose"));
        // Every kept line sits at its original offset.
        assert_eq!(
            converted.find("x <- 1L"),
            document.find("x <- 1L"),
            "{converted:?}"
        );
    }

    #[test]
    fn a_fence_for_another_language_is_prose() {
        let document = "```{python}\nx = 1\n```\n```{r}\ny <- 2L\n```\n";
        let converted = r_source_of_literate(document);
        assert!(!converted.contains("x = 1"));
        assert!(converted.contains("y <- 2L"));
    }

    #[test]
    fn sweave_chunks_convert() {
        let document = "\\section{One}\n<<setup, echo=FALSE>>=\nz <- 3L\n@\ntext\n";
        let converted = r_source_of_literate(document);
        assert_eq!(converted.len(), document.len());
        assert!(converted.contains("z <- 3L"));
        assert!(!converted.contains("section"));
    }

    #[test]
    fn multibyte_prose_keeps_offsets_aligned() {
        let document = "prosé — ünicode\n```{r}\nx <- 1L\n```\n";
        let converted = r_source_of_literate(document);
        assert_eq!(
            converted.chars().count(),
            document.chars().count(),
            "{converted:?}"
        );
        assert!(converted.contains("x <- 1L"));
    }

    #[test]
    fn an_unclosed_chunk_still_yields_its_code() {
        let document = "```{r}\nx <- 1L\n";
        assert!(r_source_of_literate(document).contains("x <- 1L"));
    }
}
