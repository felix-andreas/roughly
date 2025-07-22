# Building with Nix

This project can be built using Nix with crane for Rust projects. The build is designed to work on all supported platforms, including macOS.

## Building the package

To build the `roughly` package using Nix:

```bash
nix build .#roughly
```

Or simply:

```bash
nix build
```

The built binary will be available in `./result/bin/roughly`.

## Available packages

- `roughly` - The main R language server binary
- `default` - Alias for the `roughly` package

## Development

The existing development shell is still available:

```bash
nix develop
```

This provides all the development tools including Rust toolchain, cargo extensions, and R dependencies.

## Cross-platform support

The build includes platform-specific dependencies:

- **macOS**: Includes Security and SystemConfiguration frameworks, plus libiconv
- **Linux**: Standard build dependencies
- **Windows**: Cross-compilation support via the dev shell

## Dependencies

The Nix build includes:

- `tree-sitter` - For parsing R source files
- Platform-specific frameworks and libraries
- All Rust dependencies managed by crane

## Crane integration

This project uses [crane](https://github.com/ipetkov/crane) for efficient Rust builds in Nix:

- Dependency caching for faster incremental builds
- Clean source filtering
- Proper cross-platform support
- Integration with the existing rust-overlay setup