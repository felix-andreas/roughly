//! End-to-end tests for `roughly repl`, driving the real binary through a
//! pseudo-terminal: type input, watch R evaluate. They need a local R
//! installation, so they SKIP (loudly, but green) where none exists — CI has
//! no R, and by decision these run locally before REPL-touching changes:
//!
//! ```sh
//! cargo test -p roughly --test test_repl_e2e -- --nocapture
//! ```

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn r_available() -> bool {
    let available = std::process::Command::new("R")
        .arg("RHOME")
        .output()
        .is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("skipped: no R installation on this machine");
    }
    available
}

struct ReplSession {
    _pair: portable_pty::PtyPair,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    seen: String,
}

impl ReplSession {
    fn start() -> ReplSession {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 32,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_roughly"));
        command.arg("repl");
        let child = pair.slave.spawn_command(command).expect("spawn repl");
        let reader = pair.master.try_clone_reader().expect("pty reader");
        let writer = pair.master.take_writer().expect("pty writer");
        ReplSession {
            _pair: pair,
            child,
            reader,
            writer,
            seen: String::new(),
        }
    }

    fn send(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).expect("pty write");
        self.writer.flush().expect("pty flush");
    }

    /// Reads until the accumulated (ANSI-stripped) output contains `needle`.
    /// Panics with everything seen so far on timeout, so a failure names
    /// what the session actually did.
    fn expect(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut buffer = [0u8; 4096];
        while !self.seen.contains(needle) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; output so far:\n{}",
                self.seen
            );
            match self.reader.read(&mut buffer) {
                Ok(0) => std::thread::sleep(Duration::from_millis(20)),
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]);
                    self.seen.push_str(&strip_ansi(&chunk));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    fn quit(mut self) {
        self.send("q()\r");
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if self.child.try_wait().expect("child wait").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.child.kill().ok();
        panic!("the session did not exit after q(); output:\n{}", self.seen);
    }
}

/// Drops ESC-introduced control sequences so assertions match what a human
/// reads, not the editor's redraw traffic.
fn strip_ansi(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            stripped.push(character);
            continue;
        }
        match characters.peek() {
            // CSI: parameters then a final byte in @..~
            Some('[') => {
                characters.next();
                for terminator in characters.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&terminator) {
                        break;
                    }
                }
            }
            // OSC: terminated by BEL or ESC \
            Some(']') => {
                characters.next();
                while let Some(inner) = characters.next() {
                    if inner == '\u{7}' {
                        break;
                    }
                    if inner == '\u{1b}' && characters.peek() == Some(&'\\') {
                        characters.next();
                        break;
                    }
                }
            }
            _ => {
                characters.next();
            }
        }
    }
    stripped
}

#[test]
fn evaluates_and_autoprints() {
    if !r_available() {
        return;
    }
    let mut session = ReplSession::start();
    session.expect("Roughly R console");
    session.send("1 + 1\r");
    session.expect("[1] 2");
    session.quit();
}

#[test]
fn multiline_input_continues_and_defines() {
    if !r_available() {
        return;
    }
    let mut session = ReplSession::start();
    session.expect("Roughly R console");
    session.send("f <- function(x) {\r");
    session.send("x * 2\r");
    session.send("}\r");
    session.send("f(21)\r");
    session.expect("[1] 42");
    session.quit();
}

#[test]
fn errors_come_back_on_the_error_stream() {
    if !r_available() {
        return;
    }
    let mut session = ReplSession::start();
    session.expect("Roughly R console");
    session.send("stop(\"boom\")\r");
    session.expect("boom");
    session.send("1 + 1\r");
    session.expect("[1] 2");
    session.quit();
}

#[test]
fn ctrl_c_interrupts_evaluation() {
    if !r_available() {
        return;
    }
    let mut session = ReplSession::start();
    session.expect("Roughly R console");
    session.send("Sys.sleep(60)\r");
    std::thread::sleep(Duration::from_millis(1500));
    session.send("\u{3}");
    // The session must come back alive well before the sleep could finish.
    session.send("40 + 2\r");
    session.expect("[1] 42");
    session.quit();
}
