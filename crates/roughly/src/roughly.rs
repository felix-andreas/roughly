//! The `roughly` product surface on the greenfield stack: the CLI (`check`,
//! `fmt`, `server`, `debug`) and the language server, over the `semantics`,
//! `ide`, and `format` crates.

pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod namespace;
pub mod position;
pub mod repl_completer;
pub mod server;
pub mod stats;

/// Analysis walks recurse as deeply as the source nests, so commands and the
/// analysis worker run on threads with an explicit large stack instead of
/// whatever `ulimit -s` grants the main thread.
pub const ANALYSIS_STACK_SIZE: usize = 64 * 1024 * 1024;
