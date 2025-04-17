#![feature(let_chains)]

pub mod cli;
pub mod completions;
pub mod config;
pub mod dev;
pub mod diagnostics;
pub mod format;
pub mod index;
pub mod repl;
pub mod tree;
pub mod utils;

#[cfg(feature = "async-lsp")]
pub mod lsp_async_lsp;
#[cfg(feature = "async-lsp")]
pub use {async_lsp::lsp_types, lsp_async_lsp as lsp};

#[cfg(feature = "tower-lsp")]
mod lsp_tower_lsp;
#[cfg(feature = "tower-lsp")]
pub use {async_lsp::lsp_types, lsp_tower_lsp as lsp};
