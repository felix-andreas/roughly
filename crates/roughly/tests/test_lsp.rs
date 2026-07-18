//! Behavioral tests for the language server, driving the real `roughly
//! server` binary over LSP stdio: capability negotiation, document sync,
//! push/pull diagnostics, position encodings, and the feature endpoints.

use async_lsp::LanguageServer;
use async_lsp::concurrency::{Concurrency, ConcurrencyLayer};
use async_lsp::lsp_types::notification::{PublishDiagnostics, ShowMessage};
use async_lsp::lsp_types::request::{RegisterCapability, WorkspaceDiagnosticRefresh};
use async_lsp::lsp_types::{
    ClientCapabilities, DiagnosticClientCapabilities, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, DocumentFormattingParams, FormattingOptions,
    GeneralClientCapabilities, GotoDefinitionParams, GotoDefinitionResponse, HoverContents,
    HoverParams, InitializeParams, InitializeResult, InitializedParams, PartialResultParams,
    Position, PositionEncodingKind, PublishDiagnosticsParams, ShowMessageParams,
    TextDocumentClientCapabilities, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, Url, VersionedTextDocumentIdentifier,
    WorkDoneProgressParams, WorkspaceFolder,
};
use async_lsp::panic::{CatchUnwind, CatchUnwindLayer};
use async_lsp::router::Router;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tower::ServiceBuilder;

struct TestClientState {
    diagnostics_sender: mpsc::UnboundedSender<PublishDiagnosticsParams>,
    refresh_sender: mpsc::UnboundedSender<()>,
    messages_sender: mpsc::UnboundedSender<ShowMessageParams>,
}

struct Stop;

type TestService = CatchUnwind<Concurrency<Router<TestClientState>>>;

fn build_test_client(
    diagnostics_sender: mpsc::UnboundedSender<PublishDiagnosticsParams>,
    refresh_sender: mpsc::UnboundedSender<()>,
    messages_sender: mpsc::UnboundedSender<ShowMessageParams>,
) -> (async_lsp::MainLoop<TestService>, async_lsp::ServerSocket) {
    async_lsp::MainLoop::new_client(|_server| {
        let mut router = Router::new(TestClientState {
            diagnostics_sender,
            refresh_sender,
            messages_sender,
        });
        router.notification::<PublishDiagnostics>(|state, params| {
            state
                .diagnostics_sender
                .send(params)
                .expect("diagnostics channel closed unexpectedly");
            ControlFlow::Continue(())
        });
        router.request::<RegisterCapability, _>(|_, _| std::future::ready(Ok(())));
        router.request::<WorkspaceDiagnosticRefresh, _>(|state, _| {
            let _ = state.refresh_sender.send(());
            std::future::ready(Ok(()))
        });
        router.notification::<ShowMessage>(|state, params| {
            let _ = state.messages_sender.send(params);
            ControlFlow::Continue(())
        });
        router.event(|_, _: Stop| ControlFlow::Break(Ok(())));
        ServiceBuilder::new()
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .service(router)
    })
}

fn spawn_server(server_cwd: &Path) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_roughly"))
        .arg("server")
        .current_dir(server_cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn roughly server")
}

/// The push channel plus a stash: deferred semantic publishes for different
/// documents interleave in idle order, so a wait targeted at one URI must
/// keep — not drop — the publishes it skips for other URIs.
struct DiagnosticsChannel {
    receiver: mpsc::UnboundedReceiver<PublishDiagnosticsParams>,
    stash: Vec<PublishDiagnosticsParams>,
}

const TIMEOUT: Duration = Duration::from_secs(10);

/// The SETTLED diagnostics for `uri`: skips (and consumes) the versioned
/// first-wave publishes and returns the next version-less publish.
async fn recv_diagnostics(
    channel: &mut DiagnosticsChannel,
    uri: &Url,
    timeout_duration: Duration,
) -> PublishDiagnosticsParams {
    if let Some(index) = channel
        .stash
        .iter()
        .position(|params| params.uri == *uri && params.version.is_none())
    {
        let params = channel.stash.remove(index);
        let kept_prefix: Vec<_> = channel
            .stash
            .drain(..index)
            .filter(|stashed| stashed.uri != *uri)
            .collect();
        channel.stash.splice(..0, kept_prefix);
        return params;
    }
    tokio::time::timeout(timeout_duration, async {
        loop {
            let params = channel
                .receiver
                .recv()
                .await
                .expect("diagnostics channel closed");
            if params.uri == *uri {
                if params.version.is_none() {
                    return params;
                }
                continue;
            }
            channel.stash.push(params);
        }
    })
    .await
    .expect("timed out waiting for diagnostics")
}

/// The FIRST publish for `uri`, whichever wave it is.
async fn recv_first_diagnostics(
    channel: &mut DiagnosticsChannel,
    uri: &Url,
    timeout_duration: Duration,
) -> PublishDiagnosticsParams {
    if let Some(index) = channel.stash.iter().position(|params| params.uri == *uri) {
        return channel.stash.remove(index);
    }
    tokio::time::timeout(timeout_duration, async {
        loop {
            let params = channel
                .receiver
                .recv()
                .await
                .expect("diagnostics channel closed");
            if params.uri == *uri {
                return params;
            }
            channel.stash.push(params);
        }
    })
    .await
    .expect("timed out waiting for diagnostics")
}

struct TestContext {
    server: async_lsp::ServerSocket,
    diagnostics_receiver: DiagnosticsChannel,
    /// Wired for the diagnostic-refresh tests that follow in the full port.
    #[allow(dead_code)]
    refresh_receiver: mpsc::UnboundedReceiver<()>,
    messages_receiver: mpsc::UnboundedReceiver<ShowMessageParams>,
    mainloop_handle: tokio::task::JoinHandle<()>,
    init_result: InitializeResult,
    _temp_dir: tempfile::TempDir,
    workspace_dir: PathBuf,
}

impl TestContext {
    fn workspace_uri(&self, relative: &str) -> Url {
        Url::from_file_path(self.workspace_dir.join(relative)).expect("workspace file URI")
    }

    async fn open(&mut self, relative: &str, text: &str) -> Url {
        let uri = self.workspace_uri(relative);
        self.server
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "r".into(),
                    version: 1,
                    text: text.to_owned(),
                },
            })
            .expect("didOpen failed");
        uri
    }

    async fn shutdown(mut self) {
        let _ = self.server.shutdown(()).await;
        let _ = self.server.exit(());
        let _ = tokio::time::timeout(TIMEOUT, self.mainloop_handle).await;
    }
}

async fn setup_test_inner(
    create_r_directory: bool,
    initial_files: &[(&str, &str)],
    capabilities: ClientCapabilities,
) -> TestContext {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    // The server process runs with the temp ROOT as its cwd while the client
    // announces the `workspace` subdirectory as the workspace root, so every
    // test exercises root-from-initialize rather than root-from-cwd.
    let server_cwd = temp_dir.path().to_path_buf();
    let workspace_dir = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_dir).expect("failed to create workspace directory");
    if create_r_directory {
        std::fs::create_dir_all(workspace_dir.join("R")).expect("failed to create R directory");
    }
    for (relative_path, text) in initial_files {
        let path = workspace_dir.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent directory");
        }
        std::fs::write(path, text).expect("failed to write initial test file");
    }

    let (diagnostics_sender, diagnostics_receiver) = mpsc::unbounded_channel();
    let (refresh_sender, refresh_receiver) = mpsc::unbounded_channel();
    let (messages_sender, messages_receiver) = mpsc::unbounded_channel();
    let (mainloop, mut server) =
        build_test_client(diagnostics_sender, refresh_sender, messages_sender);

    let mut child = spawn_server(&server_cwd);
    let stdout = child.stdout.take().expect("missing stdout").compat();
    let stdin = child.stdin.take().expect("missing stdin").compat_write();
    let mainloop_handle = tokio::spawn(async move {
        if let Err(error) = mainloop.run_buffered(stdout, stdin).await {
            eprintln!("mainloop error: {error}");
        }
        drop(child);
    });

    let root_uri = Url::from_file_path(&workspace_dir).expect("invalid workspace path");
    let init_result = server
        .initialize(InitializeParams {
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: "root".into(),
            }]),
            capabilities,
            ..InitializeParams::default()
        })
        .await
        .expect("initialize failed");
    server
        .initialized(InitializedParams {})
        .expect("initialized notification failed");
    tokio::time::sleep(Duration::from_millis(200)).await;

    TestContext {
        server,
        diagnostics_receiver: DiagnosticsChannel {
            receiver: diagnostics_receiver,
            stash: Vec::new(),
        },
        refresh_receiver,
        messages_receiver,
        mainloop_handle,
        init_result,
        _temp_dir: temp_dir,
        workspace_dir,
    }
}

async fn setup_test(initial_files: &[(&str, &str)]) -> TestContext {
    setup_test_inner(true, initial_files, ClientCapabilities::default()).await
}

async fn setup_test_with_position_encodings(
    initial_files: &[(&str, &str)],
    position_encodings: &[PositionEncodingKind],
) -> TestContext {
    let capabilities = ClientCapabilities {
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(position_encodings.to_vec()),
            ..GeneralClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    setup_test_inner(true, initial_files, capabilities).await
}

async fn setup_test_with_pull_diagnostics(initial_files: &[(&str, &str)]) -> TestContext {
    let capabilities = ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            diagnostic: Some(DiagnosticClientCapabilities::default()),
            ..TextDocumentClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    setup_test_inner(true, initial_files, capabilities).await
}

fn position_params(uri: &Url, line: u32, character: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position::new(line, character),
    }
}

//
// Tests
//

#[tokio::test]
async fn initialize_reports_capabilities() {
    let context = setup_test(&[]).await;
    let capabilities = &context.init_result.capabilities;
    assert!(capabilities.hover_provider.is_some());
    assert!(capabilities.definition_provider.is_some());
    assert!(capabilities.references_provider.is_some());
    assert!(capabilities.rename_provider.is_some());
    assert!(capabilities.completion_provider.is_some());
    assert!(capabilities.document_formatting_provider.is_some());
    assert!(capabilities.document_symbol_provider.is_some());
    assert!(capabilities.inlay_hint_provider.is_some());
    assert!(capabilities.signature_help_provider.is_some());
    assert!(capabilities.semantic_tokens_provider.is_some());
    assert!(capabilities.diagnostic_provider.is_some());
    assert_eq!(
        context
            .init_result
            .server_info
            .as_ref()
            .map(|info| info.name.as_str()),
        Some("roughly")
    );
    context.shutdown().await;
}

#[tokio::test]
async fn initialize_negotiates_utf16_by_default() {
    let context = setup_test(&[]).await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF16)
    );
    context.shutdown().await;
}

#[tokio::test]
async fn initialize_negotiates_utf8_when_offered() {
    let context = setup_test_with_position_encodings(
        &[],
        &[PositionEncodingKind::UTF8, PositionEncodingKind::UTF16],
    )
    .await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF8)
    );
    context.shutdown().await;
}

#[tokio::test]
async fn diagnostics_on_open() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/bad.R", "x = 1\n").await;
    let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    assert_eq!(params.diagnostics.len(), 1, "{:?}", params.diagnostics);
    assert!(
        params.diagnostics[0]
            .message
            .contains("Use <-, not =, for assignment"),
        "{:?}",
        params.diagnostics
    );
    context.shutdown().await;
}

#[tokio::test]
async fn no_diagnostics_for_clean_file() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/clean.R", "x <- 1\n").await;
    let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    assert!(params.diagnostics.is_empty(), "{:?}", params.diagnostics);
    context.shutdown().await;
}

#[tokio::test]
async fn diagnostics_on_syntax_error() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/bad.R", "x <- (\n").await;
    let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    assert!(
        !params.diagnostics.is_empty(),
        "expected syntax diagnostics"
    );
    assert!(
        params.diagnostics[0].message.contains("unclosed `(`"),
        "{:?}",
        params.diagnostics
    );
    context.shutdown().await;
}

#[tokio::test]
async fn first_wave_publishes_syntax_errors_before_semantics() {
    let mut context = setup_test(&[]).await;
    let uri = context
        .open("R/bad.R", "x <- (\nbad_type <- 1L + \"a\"\n")
        .await;
    let first = recv_first_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    // The first wave is versioned and carries the cheap classes.
    assert_eq!(first.version, Some(1));
    assert!(
        first
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unclosed `(`")),
        "{:?}",
        first.diagnostics
    );
    context.shutdown().await;
}

#[tokio::test]
async fn burst_of_edits_settles_on_the_final_text() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/burst.R", "x <- 1\n").await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    for version in 2..=6 {
        let text = if version == 6 {
            "x = 1\n".to_owned()
        } else {
            format!("x <- {version}\n")
        };
        context
            .server
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text,
                }],
            })
            .expect("didChange failed");
    }
    // Eventually the settled set reflects the final text: one lint warning.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
        if params.diagnostics.len() == 1
            && params.diagnostics[0]
                .message
                .contains("Use <-, not =, for assignment")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never settled on the final text: {:?}",
            params.diagnostics
        );
    }
    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_type() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/hover.R", "count <- 1L\n").await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: position_params(&uri, 0, 2),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover failed")
        .expect("expected a hover");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markdown hover");
    };
    assert!(markup.value.contains("integer"), "{}", markup.value);
    context.shutdown().await;
}

#[tokio::test]
async fn hover_range_under_utf16_with_non_bmp_emoji() {
    let mut context = setup_test(&[]).await;
    // The emoji is 4 UTF-8 bytes / 2 UTF-16 units inside the string.
    let uri = context
        .open("R/emoji.R", "s <- \"😀\"\ncount <- 1L\n")
        .await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: position_params(&uri, 1, 2),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover failed")
        .expect("expected a hover");
    let range = hover.range.expect("hover range");
    assert_eq!(range.start, Position::new(1, 0));
    assert_eq!(range.end, Position::new(1, 5));
    context.shutdown().await;
}

#[tokio::test]
async fn goto_definition() {
    let mut context = setup_test(&[]).await;
    let uri = context
        .open(
            "R/def.R",
            "helper <- function(x) x\ncaller <- function() helper(1)\n",
        )
        .await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    let response = context
        .server
        .definition(GotoDefinitionParams {
            text_document_position_params: position_params(&uri, 1, 22),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("definition failed")
        .expect("expected a definition");
    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected a scalar definition");
    };
    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(0, 0));
    assert_eq!(location.range.end, Position::new(0, 6));
    context.shutdown().await;
}

#[tokio::test]
async fn formatting() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/fmt.R", "x<-1\n").await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    let edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting failed")
        .expect("expected formatting edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "x <- 1\n");
    assert_eq!(edits[0].range.start, Position::new(0, 0));
    context.shutdown().await;
}

#[tokio::test]
async fn formatting_refuses_on_syntax_errors() {
    let mut context = setup_test(&[]).await;
    let uri = context.open("R/bad.R", "x <- (\n").await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    let edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting failed");
    assert!(edits.is_none(), "{edits:?}");
    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_report_known_diagnostics() {
    let mut context = setup_test_with_pull_diagnostics(&[]).await;
    let uri = context.open("R/warn.R", "x = 1\n").await;
    let report = context
        .server
        .document_diagnostic(DocumentDiagnosticParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            identifier: Some("roughly".into()),
            previous_result_id: None,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("pull diagnostics failed");
    let DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(full)) = report
    else {
        panic!("expected a full report");
    };
    assert_eq!(full.full_document_diagnostic_report.items.len(), 1);
    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_unchanged_on_repeat() {
    let mut context = setup_test_with_pull_diagnostics(&[]).await;
    let uri = context.open("R/warn.R", "x = 1\n").await;
    let params = DocumentDiagnosticParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        identifier: Some("roughly".into()),
        previous_result_id: None,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let first = context
        .server
        .document_diagnostic(params.clone())
        .await
        .expect("pull diagnostics failed");
    let DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(full)) = first else {
        panic!("expected a full report");
    };
    let result_id = full
        .full_document_diagnostic_report
        .result_id
        .expect("result id");
    let second = context
        .server
        .document_diagnostic(DocumentDiagnosticParams {
            previous_result_id: Some(result_id),
            ..params
        })
        .await
        .expect("repeat pull failed");
    assert!(
        matches!(
            second,
            DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_))
        ),
        "expected an unchanged report"
    );
    context.shutdown().await;
}

#[tokio::test]
async fn pull_capable_client_suppresses_push() {
    let mut context = setup_test_with_pull_diagnostics(&[]).await;
    let _uri = context.open("R/warn.R", "x = 1\n").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        context.diagnostics_receiver.receiver.try_recv().is_err(),
        "push must be suppressed for pull clients"
    );
    context.shutdown().await;
}

#[tokio::test]
async fn workspace_root_comes_from_the_client_not_the_process_cwd() {
    // setup runs the server with the temp ROOT as cwd and announces
    // `workspace/` as the root; a package file under `workspace/R` must be
    // analyzed as part of the package.
    let mut context = setup_test(&[("R/lib.R", "make_count <- function() 1L\n")]).await;
    let uri = context.open("R/use.R", "total <- make_count()\n").await;
    let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    assert!(
        params.diagnostics.is_empty(),
        "cross-file resolution must succeed: {:?}",
        params.diagnostics
    );
    context.shutdown().await;
}

#[tokio::test]
async fn malformed_config_falls_back_to_defaults_and_reports() {
    let mut context = setup_test(&[("roughly.toml", "debug = 1\n")]).await;
    let message = tokio::time::timeout(TIMEOUT, context.messages_receiver.recv())
        .await
        .expect("timed out waiting for the config error message")
        .expect("messages channel closed");
    assert!(
        message.message.contains("invalid config"),
        "{}",
        message.message
    );
    // The server still works with default config.
    let uri = context.open("R/clean.R", "x <- 1\n").await;
    let params = recv_diagnostics(&mut context.diagnostics_receiver, &uri, TIMEOUT).await;
    assert!(params.diagnostics.is_empty(), "{:?}", params.diagnostics);
    context.shutdown().await;
}
