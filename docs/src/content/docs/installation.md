---
title: Installation
description: Every way to install Roughly, including the cases the quick path does not cover
---

Most people are done in one click — see the install section of
[getting started](/getting-started#install). This page is for everything else.

## VS Code

The [Roughly extension](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly)
bundles the binary for **Linux x86_64, macOS aarch64, and Windows x86_64**.

On any other architecture the extension installs but has no binary to run. Install the CLI separately
and point the extension at it:

```json
// settings.json
{
  "roughly.path": "/absolute/path/to/roughly"
}
```

Every extension setting is listed under
[editor settings](/reference/configuration#editor-settings).

## Zed

Not in Zed's extension registry yet. Until it is, install it from the repository as a dev extension:

1. Install a [Rust toolchain](https://rustup.rs) — Zed compiles dev extensions to WebAssembly itself.
2. Clone the repository.
3. Run `zed: install dev extension` from the command palette and select the `editors/zed` directory.

Zed has no built-in R support, so install the [R extension](https://zed.dev/extensions/r) first. Full
instructions, including how the extension finds the binary, are in
[`editors/zed`](https://github.com/felix-andreas/roughly/tree/main/editors/zed).

## Command line

**Prebuilt binary.** Download from
[Releases](https://github.com/felix-andreas/roughly/releases). Assets are named by Rust target triple
and each archive holds a single `roughly` binary:

| Platform | Asset |
| --- | --- |
| Linux x86_64 | `roughly-x86_64-unknown-linux-gnu.tar.gz` |
| macOS aarch64 | `roughly-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `roughly-x86_64-pc-windows-gnu.zip` |

```bash
curl -sSL https://github.com/felix-andreas/roughly/releases/download/0.3.0-alpha/roughly-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv roughly /usr/local/bin/
```

Name the tag explicitly. Every release so far is marked a pre-release, so `releases/latest/` resolves
to an older stable tag rather than the newest build.

**From source**, if you have a [Rust toolchain](https://www.rust-lang.org/tools/install):

```bash
cargo install --git https://github.com/felix-andreas/roughly roughly
```

This is also the route for architectures without a prebuilt binary.

**Planned:** a one-line installer, so neither a manual download nor a Rust toolchain is needed. No date.

## RStudio

RStudio has no language-server integration, but it can use Roughly as its external formatter:

1. **Tools → Global Options → Code → Formatting → Format with an External Tool**, and set the reformat
   command to `path/to/roughly fmt`.
2. For format-on-save, **Tools → Global Options → Code → Saving** and tick **Reformat documents on
   save**.

Type checking and code analysis are not available inside RStudio. Run `roughly check` in a terminal, or
in [CI](/guides/continuous-integration).

## Verifying

```bash
roughly --version
```

Then run it on a project — [getting started](/getting-started) shows what a first run looks like.
