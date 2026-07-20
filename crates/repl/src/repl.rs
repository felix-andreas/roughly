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

/// Starts the interactive console on the CALLING thread and only returns
/// when the R session ends (`q()`, EOF). R assumes it owns the thread it
/// initializes on — run this from `main` without spawning.
pub fn run() -> Result<(), ReplError> {
    let api = libr::load()?;
    console::run(api)
}
