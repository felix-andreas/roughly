#![feature(let_chains)]

pub mod cli;
pub mod completions;
pub mod config;
pub mod dev;
pub mod diagnostics;
pub mod format;
pub mod index;
pub mod tree;
pub mod utils;

#[cfg(feature = "async-lsp")]
pub mod server;

#[cfg(feature = "async-lsp")]
pub use async_lsp::lsp_types;
