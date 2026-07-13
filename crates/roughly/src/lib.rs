pub mod cli;
pub(crate) mod config;
pub(crate) mod diagnostics;
pub mod format;
pub mod index;
pub(crate) mod position;
pub(crate) mod server;
pub mod stats;
pub mod symbols;
pub mod tree;
pub mod utils;

pub use async_lsp::lsp_types;

/// Stack size for every thread that walks syntax trees (the engine worker and the CLI command
/// thread). The formatter and the IDE walks are recursive over the CST, whose depth is naturally
/// bounded by the grammar's error-recovery onset (~1,100 nested levels — deeper input parses into
/// ERROR trees, which only the iterative diagnostic walks visit); the deepest such tree needs a few
/// MiB of stack, and 64 MiB of reserved (not committed) address space gives an order-of-magnitude
/// margin over that bound.
pub const ANALYSIS_STACK_SIZE: usize = 64 * 1024 * 1024;
