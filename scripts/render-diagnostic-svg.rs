#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"
---
//! Renders a real `ry check` run to a theme-aware SVG for the README.
//!
//! GitHub strips ANSI escapes from fenced code blocks, so the only way to show
//! the diagnostics in the colours a terminal shows them is an image. GitHub does
//! honour a `<style>` block *inside* an embedded SVG, including
//! `prefers-color-scheme`, so one file serves both the light and the dark theme.
//!
//! The point of generating rather than hand-drawing it: the picture is produced
//! by running the tool, so it cannot drift from what the tool actually prints.
//! Re-run this whenever diagnostic rendering changes.
//!
//! Usage: `scripts/render-diagnostic-svg.rs <fixture.R> <out.svg>`
//! The fixture's directory needs a `ry.toml`; the binary must be built first.
//!
//! The README's image is `.github/diagnostic.svg`, rendered from a file
//! named `setup.R` holding exactly the snippet the README shows above it,
//! beside a `ry.toml` (an empty one is enough). Keep the two in step: the
//! picture is the one thing in that document that cannot be verified by
//! reading it, so a stale render is invisible until someone notices the
//! wording no longer matches the tool.

use std::fmt::Write as _;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let (Some(fixture), Some(output)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: render-diagnostic-svg.rs <fixture.R> <out.svg>");
        std::process::exit(2);
    };

    // `script` gives the child a pty, which is what makes the tool colour its
    // output — piping it would produce plain text, exactly as it should.
    let rendered = Command::new("script")
        .args(["-qec", &format!("ry check {fixture}"), "/dev/null"])
        .output()?;
    let text = String::from_utf8_lossy(&rendered.stdout).replace('\r', "");
    if text.trim().is_empty() {
        return Err(format!("`ry check {fixture}` printed nothing — is `ry` on PATH?").into());
    }

    std::fs::write(&output, svg(&text))?;
    println!("wrote {output}");
    Ok(())
}

/// One SVG holding the rendered lines, styled for both GitHub themes.
fn svg(text: &str) -> String {
    let lines: Vec<Vec<Span>> = text
        .lines()
        .map(parse_ansi)
        .filter(|spans| !spans.is_empty())
        .filter(|spans| {
            // The trailing "N problems in M files" run summary is terminal
            // chrome, not the diagnostic — the image shows only the finding.
            // The locus line stays: it opens the snippet frame the gutter
            // hangs from, so dropping it would leave the snippet headless.
            let plain: String = spans.iter().map(|span| span.text.as_str()).collect();
            let plain = plain.trim();
            !(plain.starts_with(|c: char| c.is_ascii_digit()) && plain.contains(" problem"))
        })
        .collect();

    // A monospace advance of 0.6em is the usual approximation and only decides
    // the viewport, which is padded anyway.
    let columns = lines
        .iter()
        .map(|spans| spans.iter().map(|span| span.text.chars().count()).sum())
        .max()
        .unwrap_or(0);
    let width = (columns as f32 * 8.4).max(360.0) + 40.0;
    let height = lines.len() as f32 * 22.0 + 30.0;
    // The description is what a screen reader announces and what search
    // indexes, so it carries the diagnostic as plain text — escapes stripped.
    let plain: String = lines
        .iter()
        .map(|spans| {
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");

    let mut out = String::new();
    let _ = write!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:.0} {height:.0}" width="{width:.0}" height="{height:.0}" role="img" aria-labelledby="title desc">
  <title id="title">A ry diagnostic</title>
  <desc id="desc">{}</desc>
  <style>
    .bg {{ fill: #ffffff; stroke: #d0d7de; }}
    .fg {{ fill: #1f2328; }}
    .red {{ fill: #cf222e; }}
    .blue {{ fill: #0969da; }}
    .cyan {{ fill: #1b7c83; }}
    .magenta {{ fill: #8250df; }}
    .green {{ fill: #1a7f37; }}
    .yellow {{ fill: #9a6700; }}
    .dim {{ fill: #59636e; }}
    @media (prefers-color-scheme: dark) {{
      .bg {{ fill: #0d1117; stroke: #30363d; }}
      .fg {{ fill: #e6edf3; }}
      .red {{ fill: #ff7b72; }}
      .blue {{ fill: #79c0ff; }}
      .cyan {{ fill: #76e3ea; }}
      .magenta {{ fill: #d2a8ff; }}
      .green {{ fill: #7ee787; }}
      .yellow {{ fill: #e3b341; }}
      .dim {{ fill: #9198a1; }}
    }}
    text {{ font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace; font-size: 14px; white-space: pre; }}
  </style>
  <rect class="bg" x="0.5" y="0.5" width="{:.0}" height="{:.0}" rx="6"/>
"##,
        escape(&plain),
        width - 1.0,
        height - 1.0,
    );

    for (row, spans) in lines.iter().enumerate() {
        let y = 30.0 + row as f32 * 22.0;
        let _ = write!(out, r#"  <text x="16" y="{y:.0}">"#);
        let mut column = 0usize;
        for span in spans {
            let _ = write!(
                out,
                r#"<tspan x="{:.1}" class="{}"{}{}>{}</tspan>"#,
                16.0 + column as f32 * 8.4,
                span.class,
                if span.bold { r#" font-weight="600""# } else { "" },
                if span.underline { r#" text-decoration="underline""# } else { "" },
                escape(&span.text)
            );
            column += span.text.chars().count();
        }
        out.push_str("</text>\n");
    }
    out.push_str("</svg>\n");
    out
}

struct Span {
    text: String,
    class: &'static str,
    bold: bool,
    underline: bool,
}

/// Splits one line into coloured spans.
///
/// One escape carries several parameters — `\e[36;1;4m` is cyan, bold and
/// underlined at once — so each is applied in turn rather than matched as a
/// whole. Handling only the single-parameter forms left every compound one
/// falling through to the default colour, which is a monochrome picture that
/// still looks plausible: the reason the render is worth an eye after every
/// change to the reporter's palette.
fn parse_ansi(line: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let (mut class, mut bold, mut underline) = ("fg", false, false);
    let mut current = String::new();
    let mut rest = line;

    while let Some(start) = rest.find('\u{1b}') {
        if !rest[..start].is_empty() {
            current.push_str(&rest[..start]);
        }
        let Some(end) = rest[start..].find('m') else { break };
        let code = &rest[start + 2..start + end];
        if !current.is_empty() {
            spans.push(Span { text: std::mem::take(&mut current), class, bold, underline });
        }
        for parameter in code.split(';') {
            match parameter {
                "0" | "" => (class, bold, underline) = ("fg", false, false),
                "1" => bold = true,
                "4" => underline = true,
                "2" | "90" => class = "dim",
                "31" => class = "red",
                "32" => class = "green",
                "33" => class = "yellow",
                "34" => class = "blue",
                "35" => class = "magenta",
                "36" => class = "cyan",
                _ => (),
            }
        }
        rest = &rest[start + end + 1..];
    }
    current.push_str(rest);
    if !current.trim().is_empty() {
        spans.push(Span { text: current, class, bold, underline });
    }
    spans
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
