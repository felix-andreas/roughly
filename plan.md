# LSP End-to-End Testing Plan

## Goal

Add end-to-end tests that spawn the real `roughly server` binary as a subprocess, communicate with it over the LSP protocol, and assert on responses and notifications. This tests the full stack: process startup, config loading, workspace indexing, stdio framing, the tower service layers, and all handler logic wired together.

## Approach: `async-lsp` client-side `MainLoop`

The `async-lsp` crate (already a dependency) provides `MainLoop::new_client`, which creates a typed LSP client. This handles all JSON-RPC framing, message ID correlation, and deserialization. We get a `ServerSocket` with strongly-typed methods for every LSP request/notification — no manual JSON-RPC construction needed.

Reference: see `async-lsp`'s own `examples/client_trait.rs` and `examples/client_builder.rs` which demonstrate this exact pattern (spawning a language server subprocess and talking to it via `MainLoop::new_client`).

## Architecture

### Components

1. **Test client state** — A struct that implements notification handlers via `async-lsp`'s `Router` builder API. It collects server→client notifications (primarily `publishDiagnostics`) into channels for test assertions.

2. **`ServerSocket`** — Returned by `MainLoop::new_client`. Provides typed async methods: `.initialize()`, `.did_open()`, `.formatting()`, `.definition()`, `.completion()`, `.signature_help()`, `.references()`, `.rename()`, `.shutdown()`, `.exit()`, etc.

3. **Child process** — The `roughly server` binary, spawned via `tokio::process::Command` with piped stdin/stdout.

4. **Temp workspace** — Each test creates a `tempfile::tempdir()`, writes `.R` files and optionally a `roughly.toml` into it, and spawns the server with `current_dir` set to the temp dir.

### Wiring

```
                    ┌──────────────────────┐
                    │   tokio::spawn       │
                    │   mainloop           │
                    │   .run_buffered(     │
  child.stdout ──────►  stdout,           │
  child.stdin  ◄──────  stdin)            │
                    └──────────────────────┘
                              │
               ┌──────────────┴──────────────┐
               │                             │
        ServerSocket                  Router<TestClientState>
        (send requests)              (receive notifications)
               │                             │
         test body                  mpsc channel → test body
    server.initialize()             diagnostics_receiver.recv()
    server.did_open()
    server.formatting()
    ...
```

The `async-lsp` `MainLoop` sits in the middle, multiplexing the bidirectional JSON-RPC stream. The test body uses `ServerSocket` to send requests and awaits typed responses. Server-initiated notifications flow through the `Router` into channels that the test body polls.

### IO compat layer

`async-lsp` uses `futures::io::{AsyncRead, AsyncWrite}`. Tokio's child process streams implement `tokio::io::{AsyncRead, AsyncWrite}`. Use `tokio-util`'s compat adapters (already a workspace dependency) to bridge:

```rust
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
let stdout = child.stdout.take().unwrap().compat();
let stdin = child.stdin.take().unwrap().compat_write();
```

This is the same pattern already used in `server.rs` for the `cfg(not(unix))` branch.

## File location

`crates/roughly/tests/test_lsp.rs` — a single integration test file, consistent with the existing `test_format.rs`, `test_index.rs`, `test_tree.rs` pattern.

## Dependencies

Add to `[dev-dependencies]` in `crates/roughly/Cargo.toml`:

```toml
tempfile = "3"
```

Everything else is already available as workspace dependencies: `async-lsp`, `tokio`, `tokio-util`, `futures`, `tower`, `serde_json`. These need to be added to `[dev-dependencies]` since they are currently only in `[dependencies]` (and the test is an integration test which links against the library, not the binary's own dependency set):

```toml
[dev-dependencies]
async-lsp.workspace = true
futures.workspace = true
tokio.workspace = true
tokio-util.workspace = true
tower.workspace = true
tempfile = "3"
```

Note: `serde_json` is already in `[dependencies]` so it's available. `insta` and `regex` stay as they are.

## Test harness design

### Helper: `build_test_client`

A function that creates the `MainLoop` and `ServerSocket`, taking a channel sender for diagnostics:

```rust
struct TestClientState {
    diagnostics_sender: tokio::sync::mpsc::UnboundedSender<PublishDiagnosticsParams>,
}

struct Stop;

fn build_test_client(
    diagnostics_sender: tokio::sync::mpsc::UnboundedSender<PublishDiagnosticsParams>,
) -> (MainLoop<impl LspService>, ServerSocket) {
    MainLoop::new_client(|_| {
        let mut router = Router::new(TestClientState { diagnostics_sender });

        router.notification::<notification::PublishDiagnostics>(|state, params| {
            state.diagnostics_sender.send(params).ok();
            ControlFlow::Continue(())
        });

        router.request::<request::RegisterCapability, _>(|_, _| {
            std::future::ready(Ok(()))
        });

        router.notification::<notification::ShowMessage>(|_, _| ControlFlow::Continue(()));

        ServiceBuilder::new()
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .service(router)
    })
}
```

### Helper: `spawn_server`

Spawns the roughly binary and returns the child process:

```rust
fn spawn_server(workspace_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_roughly"))
        .arg("server")
        .current_dir(workspace_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn roughly server")
}
```

`env!("CARGO_BIN_EXE_roughly")` is resolved at compile time by Cargo for integration tests in the same package. Cargo automatically builds the binary before running the test.

### Helper: `recv_diagnostics`

Wait for a diagnostics notification for a specific URI, with a timeout:

```rust
async fn recv_diagnostics(
    receiver: &mut mpsc::UnboundedReceiver<PublishDiagnosticsParams>,
    uri: &Url,
    timeout_duration: Duration,
) -> PublishDiagnosticsParams {
    let deadline = tokio::time::timeout(timeout_duration, async {
        loop {
            let params = receiver.recv().await.expect("diagnostics channel closed");
            if params.uri == *uri {
                return params;
            }
        }
    })
    .await
    .expect("timed out waiting for diagnostics")
}
```

### Test lifecycle pattern

Every test follows this pattern:

1. Create temp dir, write files
2. Spawn server
3. Build test client, wire up mainloop
4. `server.initialize(...)` → assert capabilities
5. `server.initialized(...)`
6. Perform test actions (open/change docs, request formatting/definition/etc.)
7. Assert on results
8. `server.shutdown(())` + `server.exit(())`
9. Emit `Stop` event to break the client mainloop
10. Temp dir drops automatically

## Handling `register_capability`

During `initialized`, the roughly server spawns a background task that sends a `client/registerCapability` request to the client to register file watchers. The test client must handle this request — use `router.request::<RegisterCapability, _>(|_, _| std::future::ready(Ok(())))`. If left unhandled, the router returns an error, which the server handles gracefully (logs a warning and continues), but handling it cleanly is better.

## Tests to implement

### 1. `initialize_reports_capabilities`

Verify the server reports the expected capabilities in its `InitializeResult`:
- `text_document_sync` is set (incremental)
- `document_formatting_provider` is true
- `definition_provider` is true
- `completion_provider` is present with trigger characters `$`, `@`, `:`
- `document_symbol_provider` is true
- `signature_help_provider` is present with trigger characters `(`, `,`

No files needed, just initialize and inspect the result.

### 2. `diagnostics_on_open`

Write a file with lint issues (e.g. `x <- T\ny = 1\n`), open it via `didOpen`, wait for `publishDiagnostics`. Assert:
- Diagnostics contain a warning about `T` vs `TRUE`
- Diagnostics contain a warning about `=` vs `<-`

### 3. `no_diagnostics_for_clean_file`

Open a well-formed file (`x <- 1\ny <- x + 2\n`). Wait for diagnostics. Assert the diagnostics list is empty.

### 4. `diagnostics_on_syntax_error`

Open a file with a syntax error (e.g. `f(\n` — missing closing paren). Assert diagnostics contain an error about a missing closing delimiter.

### 5. `formatting`

Open an unformatted file (`x<-1\ny  <-  2\n`). Send `textDocument/formatting`. Assert the returned `TextEdit` contains properly formatted code (`x <- 1\ny <- 2\n`).

### 6. `goto_definition`

Open a file:
```r
foo <- function(x) x
bar <- foo(1)
```
Send `textDocument/definition` with cursor on `foo` in line 2. Assert the returned location points to line 0, col 0.

### 7. `completion`

Open a file:
```r
my_function <- function(x) x
my_f
```
Send `textDocument/completion` at the end of `my_f` on line 2. Assert the completion list contains `my_function`.

### 8. `document_symbols`

Open a file with function definitions. Send `textDocument/documentSymbol`. Assert the returned symbols contain the expected function names.

### 9. `signature_help`

Open a file:
```r
f <- function(x, y, z) x + y + z
f(1, 2, 3)
```
Send `textDocument/signatureHelp` inside the call arguments. Assert the returned signature label is `f(x, y, z)` and the active parameter index is correct.

### 10. `config_indent_width`

Write a `roughly.toml` with `indent-width = 4` into the workspace. Open a file with a function body. Request formatting. Assert the output uses 4-space indentation.

## Notes

- Use `#[tokio::test]` for all tests since we need an async runtime.
- Use generous timeouts (5s) for `recv_diagnostics` to avoid flakiness on slow CI.
- `stderr` from the server is inherited (`Stdio::inherit()`) so server logs appear in test output for debugging. Alternatively, pipe to `/dev/null` for cleaner output and only inherit when debugging.
- The server sets `base_path` to `current_dir().join("R")` and indexes `.R` files in that directory on startup. Tests that need workspace-level features (cross-file go-to-definition, workspace symbols) should write files into an `R/` subdirectory inside the temp dir.
- `kill_on_drop(true)` on the child process ensures cleanup even if a test panics.