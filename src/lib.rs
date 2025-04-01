#![feature(let_chains)]

pub mod cli;
pub mod completions;
pub mod config;
pub mod dev;
pub mod diagnostics;
pub mod format;
pub mod index;
pub mod lsp;
pub mod tree;
pub mod utils;

use tower_lsp_server::lsp_types;
