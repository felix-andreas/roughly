//! An interactive R console with Roughly's language smarts, embedding R
//! WITHOUT a build-time link dependency: the R shared library is located and
//! `dlopen`ed at runtime, and every C-API symbol is resolved by name
//! (`libr`). The design record lives in the repository's agent memory
//! (`repl-design.md`); the short version:
//!
//! - R owns the calling thread and runs its REAL main loop
//!   (`run_Rmainloop`); the console lives inside R's `ReadConsole` callback,
//!   where a `reedline` editor collects input, Roughly's own parser decides
//!   completeness (continuation prompts), and finished input is fed back to
//!   R through the console buffer for R to parse, evaluate, and autoprint.
//! - Interrupts flow through `R_interrupts_pending`: Ctrl-C during
//!   evaluation is a SIGINT our handler translates into R's cooperative
//!   flag; Ctrl-C at the prompt just clears the line (reedline).
//! - Nothing here requires R at build time; a missing R at RUN time is a
//!   clear, actionable error.
//!
//! Unix only for now: the Windows embedding surface (`R_SetParams`,
//! `Rstart`, the DLL sibling set) is documented in the design record and
//! deliberately deferred.

pub mod console;
pub mod libr;

use std::fmt;

/// Everything that can stop the REPL from starting: no R on the machine, an
/// R built without `--enable-R-shlib`, a shared library that loads but lacks
/// a required symbol. Each carries the fix in its message.
#[derive(Debug)]
pub struct ReplError(pub String);

impl fmt::Display for ReplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReplError {}

/// The console's editing keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keybindings {
    #[default]
    Emacs,
    Vi,
}

/// How a session starts: interactively, with a script pre-loaded before the
/// first prompt, or as a batch run that ends at the script's end.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub keybindings: Keybindings,
    /// A script fed to R before any prompt.
    pub file: Option<std::path::PathBuf>,
    /// End the session when the script is consumed instead of prompting
    /// (`roughly run`): a top-level error quits with a failing status, so the
    /// process exit code reflects the script's outcome.
    pub batch: bool,
}

/// Starts the session on the CALLING thread and only returns when R ends
/// (`q()`, EOF, or the end of a batch script). R assumes it owns the thread
/// it initializes on — run this from `main` without spawning.
pub fn run(options: RunOptions) -> Result<(), ReplError> {
    let api = libr::load()?;
    console::run(api, options)
}
