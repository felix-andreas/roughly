<div align="center">

<img height="128px" src="../../docs/public/logo.svg" />

# Rofy

*An experimental interactive R REPL written in Rust.*

</div>

---

## ✨ Features

- **Multiline editing**: Supports writing and editing multi-line R code.
- **Command history**: Recall previous commands using <kbd>Ctrl</kbd> + <kbd>R</kbd> (reverse search).
- **Vim mode**: Optional Vim keybindings available with `rofy --vi`.
- **Syntax highlighting**: Provides syntax highlighting for R code.

---

## 🚀 Quick Start

### Download Binary (Recommended)

Download the pre-built binary for your platform from the [releases page](https://github.com/felix-andreas/roughly/releases).

### Build from Source

Alternatively, build from source (requires the Rust nightly):

```sh
cargo build -p rofy --release
```