//! The shipped artifacts carry the version in three places, and nothing else
//! keeps them in step. This is the thing that notices.
//!
//! The workspace `Cargo.toml` is the source of truth. The Zed manifest carries
//! it verbatim; the VS Code manifest carries it with any prerelease suffix
//! removed, because that manifest's version has to be a plain `major.minor.patch`.
//! Both derivations are mechanical, so a mismatch is always a stale file rather
//! than a judgement call — and the failure message says what to write.

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the repository root")
        .to_owned()
}

fn read(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// The version every shipped artifact derives from.
fn workspace_version() -> String {
    let manifest: toml::Value = read("Cargo.toml").parse().expect("the workspace manifest");
    manifest["workspace"]["package"]["version"]
        .as_str()
        .expect("workspace.package.version is a string")
        .to_owned()
}

#[test]
fn the_zed_extension_carries_the_workspace_version() {
    let expected = workspace_version();
    let manifest: toml::Value = read("editors/zed/extension.toml")
        .parse()
        .expect("the Zed manifest");
    let found = manifest["version"].as_str().expect("version is a string");
    assert_eq!(
        found, expected,
        "editors/zed/extension.toml is stale: write `version = \"{expected}\"`"
    );
}

#[test]
fn the_vs_code_extension_carries_the_release_without_its_prerelease_suffix() {
    let version = workspace_version();
    let expected = version
        .split_once('-')
        .map_or(&version[..], |(release, _)| release);
    let manifest: serde_json::Value =
        serde_json::from_str(&read("editors/code/package.json")).expect("the VS Code manifest");
    let found = manifest["version"].as_str().expect("version is a string");
    assert_eq!(
        found, expected,
        "editors/code/package.json is stale: write `\"version\": \"{expected}\"` \
         (the workspace is at {version}; this manifest drops the prerelease suffix)"
    );
}
