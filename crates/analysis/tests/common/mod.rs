// Each integration-test binary that declares `mod common;` only exercises part of this module, so
// the unused halves are reported as dead code per binary even though every item is used somewhere.
#![allow(dead_code)]

use std::path::PathBuf;

const LINES_PER_ITEM: usize = 5;

pub fn generate_package(file_count: usize, items_per_file: usize) -> Vec<(PathBuf, String)> {
    (0..file_count)
        .map(|file_index| {
            (
                file_path(file_index),
                generate_file(file_index, items_per_file, false),
            )
        })
        .collect()
}

pub fn lines_per_file(items_per_file: usize) -> usize {
    items_per_file * LINES_PER_ITEM
}

pub fn total_lines(files: &[(PathBuf, String)]) -> usize {
    files.iter().map(|(_, source)| source.lines().count()).sum()
}

pub fn file_path(file_index: usize) -> PathBuf {
    PathBuf::from(format!("/pkg/R/file_{file_index:05}.R"))
}

// `braced_body` wraps each function body in `{ ... }`, which changes body source without touching the
// exported interface, so callers can simulate a body-only edit that should recheck only one document.
pub fn generate_file(file_index: usize, items_per_file: usize, braced_body: bool) -> String {
    let dependency_file = file_index.saturating_sub(1);
    let mut source = String::new();
    for item_index in 0..items_per_file {
        let body = if braced_body {
            format!("{{ count + {item_index}L }}")
        } else {
            format!("count + {item_index}L")
        };
        source.push_str("#: fn(count: integer) -> integer\n");
        source.push_str(&format!(
            "fn_{file_index}_{item_index} <- function(count) {body}\n"
        ));
        source.push_str(&format!(
            "id_{file_index}_{item_index} <- function(value) value\n"
        ));
        source.push_str(&format!(
            "const_{file_index}_{item_index} <- {item_index}L\n"
        ));
        source.push_str(&format!(
            "use_{file_index}_{item_index} <- fn_{dependency_file}_{item_index}(2L)\n"
        ));
    }
    source
}

// Like `generate_package`, but every item additionally references embedded base stub names
// (`length`/`seq_len`/`seq_along`/`nchar`/`toupper`/`T`/`F`/`pi`), so the stub corpus is actually
// seeded into the inference template and resolved on every recheck. The LT2 zero-per-edit-cost
// benchmark uses this to compare recheck time with the real stub corpus against an empty one.
pub fn generate_package_with_base_refs(
    file_count: usize,
    items_per_file: usize,
) -> Vec<(PathBuf, String)> {
    (0..file_count)
        .map(|file_index| {
            (
                file_path(file_index),
                generate_file_with_base_refs(file_index, items_per_file, false),
            )
        })
        .collect()
}

pub fn generate_file_with_base_refs(
    file_index: usize,
    items_per_file: usize,
    braced_body: bool,
) -> String {
    let dependency_file = file_index.saturating_sub(1);
    let mut source = String::new();
    for item_index in 0..items_per_file {
        // `length(seq_len(count))` resolves two stub schemes and types back to `integer`, so the body
        // exercises stub resolution while keeping the declared `-> integer` interface honest.
        let body = if braced_body {
            "{ count + length(seq_len(count)) }".to_string()
        } else {
            "count + length(seq_len(count))".to_string()
        };
        source.push_str("#: fn(count: integer) -> integer\n");
        source.push_str(&format!(
            "fn_{file_index}_{item_index} <- function(count) {body}\n"
        ));
        source.push_str(&format!(
            "id_{file_index}_{item_index} <- function(value) value\n"
        ));
        source.push_str(&format!(
            "const_{file_index}_{item_index} <- {item_index}L\n"
        ));
        source.push_str(&format!(
            "use_{file_index}_{item_index} <- fn_{dependency_file}_{item_index}(2L)\n"
        ));
        source.push_str(&format!(
            "blen_{file_index}_{item_index} <- length(seq_along(c(1L, 2L)))\n"
        ));
        source.push_str(&format!(
            "bchr_{file_index}_{item_index} <- nchar(toupper(\"x\"))\n"
        ));
        source.push_str(&format!("bflag_{file_index}_{item_index} <- T\n"));
        source.push_str(&format!("bfalse_{file_index}_{item_index} <- F\n"));
        source.push_str(&format!("bpi_{file_index}_{item_index} <- pi\n"));
    }
    source
}
