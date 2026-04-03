use {
    async_lsp::{
        LanguageServer,
        concurrency::{Concurrency, ConcurrencyLayer},
        lsp_types::{
            CompletionParams, CompletionResponse, DidChangeWatchedFilesParams,
            DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams,
            DocumentSymbolResponse, FileChangeType, FileEvent, FormattingOptions,
            GotoDefinitionParams, GotoDefinitionResponse, HoverContents, HoverParams,
            HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
            PartialResultParams, Position, PublishDiagnosticsParams, TextDocumentIdentifier,
            TextDocumentItem, TextDocumentPositionParams, Url, WorkDoneProgressParams,
            WorkspaceFolder,
            notification::{PublishDiagnostics, ShowMessage},
            request::RegisterCapability,
        },
        panic::{CatchUnwind, CatchUnwindLayer},
        router::Router,
    },
    std::{
        ops::ControlFlow,
        path::{Path, PathBuf},
        process::Stdio,
        time::Duration,
    },
    tokio::sync::mpsc,
    tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt},
    tower::ServiceBuilder,
};

struct TestClientState {
    diagnostics_sender: mpsc::UnboundedSender<PublishDiagnosticsParams>,
}

struct Stop;

type TestService = CatchUnwind<Concurrency<Router<TestClientState>>>;

fn build_test_client(
    diagnostics_sender: mpsc::UnboundedSender<PublishDiagnosticsParams>,
) -> (async_lsp::MainLoop<TestService>, async_lsp::ServerSocket) {
    async_lsp::MainLoop::new_client(|_server| {
        let mut router = Router::new(TestClientState { diagnostics_sender });

        router.notification::<PublishDiagnostics>(|state, params| {
            state
                .diagnostics_sender
                .send(params)
                .expect("diagnostics channel closed unexpectedly");
            ControlFlow::Continue(())
        });

        router.request::<RegisterCapability, _>(|_, _| std::future::ready(Ok(())));

        router.notification::<ShowMessage>(|_, _| ControlFlow::Continue(()));

        router.event(|_, _: Stop| ControlFlow::Break(Ok(())));

        ServiceBuilder::new()
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .service(router)
    })
}

fn spawn_server_with_experimental_features(
    workspace_dir: &Path,
    experimental_features: &[&str],
) -> tokio::process::Child {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_roughly"));
    command.arg("server");
    if !experimental_features.is_empty() {
        command.arg("--experimental-features");
        command.arg(experimental_features.join(" "));
    }
    command
        .current_dir(workspace_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn roughly server")
}

async fn recv_diagnostics(
    receiver: &mut mpsc::UnboundedReceiver<PublishDiagnosticsParams>,
    uri: &Url,
    timeout_duration: Duration,
) -> PublishDiagnosticsParams {
    tokio::time::timeout(timeout_duration, async {
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

async fn drain_diagnostics(receiver: &mut mpsc::UnboundedReceiver<PublishDiagnosticsParams>) {
    loop {
        match receiver.try_recv() {
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(5);

struct TestContext {
    server: async_lsp::ServerSocket,
    diagnostics_receiver: mpsc::UnboundedReceiver<PublishDiagnosticsParams>,
    mainloop_handle: tokio::task::JoinHandle<()>,
    init_result: InitializeResult,
    _temp_dir: tempfile::TempDir,
    workspace_dir: PathBuf,
}

async fn setup_test_with_r_dir(
    create_r_directory: bool,
    initial_files: &[(&str, &str)],
) -> TestContext {
    setup_test_with_r_dir_and_features(create_r_directory, initial_files, &[]).await
}

async fn setup_test_with_r_dir_and_features(
    create_r_directory: bool,
    initial_files: &[(&str, &str)],
    experimental_features: &[&str],
) -> TestContext {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let workspace_dir = temp_dir.path().to_path_buf();
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
    let (mainloop, mut server) = build_test_client(diagnostics_sender);

    let mut child = spawn_server_with_experimental_features(&workspace_dir, experimental_features);
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
            ..InitializeParams::default()
        })
        .await
        .expect("initialize failed");

    server
        .initialized(InitializedParams {})
        .expect("initialized notification failed");

    // Give the server a moment to process the initialized notification and
    // register file watchers.
    tokio::time::sleep(Duration::from_millis(200)).await;

    TestContext {
        server,
        diagnostics_receiver,
        mainloop_handle,
        init_result,
        _temp_dir: temp_dir,
        workspace_dir,
    }
}

async fn setup_test(initial_files: &[(&str, &str)]) -> TestContext {
    setup_test_with_r_dir(true, initial_files).await
}

async fn setup_test_with_features(
    initial_files: &[(&str, &str)],
    experimental_features: &[&str],
) -> TestContext {
    setup_test_with_r_dir_and_features(true, initial_files, experimental_features).await
}

impl TestContext {
    async fn shutdown(mut self) {
        self.server.shutdown(()).await.expect("shutdown failed");
        self.server.exit(()).expect("exit failed");
        self.server.emit(Stop).expect("emit Stop failed");
        let _ = tokio::time::timeout(Duration::from_secs(3), self.mainloop_handle).await;
    }

    fn file_uri(&self, relative_path: &str) -> Url {
        Url::from_file_path(self.workspace_dir.join(relative_path)).expect("invalid file path")
    }

    async fn open_file(&mut self, uri: &Url, text: &str) {
        self.server
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "r".into(),
                    version: 0,
                    text: text.into(),
                },
            })
            .expect("did_open failed");
    }

    fn notify_watched_file_changed(&mut self, relative_path: &str, change_type: FileChangeType) {
        self.server
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent {
                    uri: self.file_uri(relative_path),
                    typ: change_type,
                }],
            })
            .expect("did_change_watched_files failed");
    }
}

#[tokio::test]
async fn initialize_reports_capabilities() {
    let context = setup_test(&[]).await;

    let capabilities = &context.init_result.capabilities;

    let sync = capabilities
        .text_document_sync
        .as_ref()
        .expect("missing text_document_sync");
    let sync_options = match sync {
        async_lsp::lsp_types::TextDocumentSyncCapability::Options(options) => options,
        _ => panic!("expected TextDocumentSyncOptions"),
    };
    assert_eq!(
        sync_options.change,
        Some(async_lsp::lsp_types::TextDocumentSyncKind::INCREMENTAL)
    );

    let formatting = &capabilities.document_formatting_provider;
    assert!(
        matches!(formatting, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected document_formatting_provider to be true"
    );

    let definition = &capabilities.definition_provider;
    assert!(
        matches!(definition, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected definition_provider to be true"
    );

    let completion = capabilities
        .completion_provider
        .as_ref()
        .expect("missing completion_provider");
    let trigger_chars = completion
        .trigger_characters
        .as_ref()
        .expect("missing trigger_characters");
    assert!(trigger_chars.contains(&"$".into()));
    assert!(trigger_chars.contains(&"@".into()));
    assert!(trigger_chars.contains(&":".into()));

    let doc_symbol = &capabilities.document_symbol_provider;
    assert!(
        matches!(doc_symbol, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected document_symbol_provider to be true"
    );

    let hover = &capabilities.hover_provider;
    assert!(
        hover.is_none(),
        "expected hover_provider to be absent by default"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn initialize_reports_hover_capability_when_enabled() {
    let context = setup_test_with_features(&[], &["hovering"]).await;

    let hover = &context.init_result.capabilities.hover_provider;
    assert!(
        matches!(hover, Some(HoverProviderCapability::Simple(true))),
        "expected hover_provider to be true when hovering is enabled"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn initialize_without_r_directory() {
    let mut context = setup_test_with_r_dir(false, &[]).await;

    let capabilities = &context.init_result.capabilities;
    assert!(
        capabilities.text_document_sync.is_some(),
        "expected initialize to succeed without an R directory"
    );

    std::fs::create_dir_all(context.workspace_dir.join("R")).expect("failed to create R directory");
    std::fs::write(context.workspace_dir.join("R/created_later.R"), "x <- T\n")
        .expect("failed to write test file");
    context.notify_watched_file_changed("R/created_later.R", FileChangeType::CREATED);

    let file_uri = context.file_uri("R/created_later.R");
    context.open_file(&file_uri, "x <- T\n").await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "expected diagnostics after creating R directory and file, got: {:?}",
        diagnostics.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_identifier_name_without_debug_by_default() {
    let mut context = setup_test_with_features(&[], &["hovering"]).await;

    let file_uri = context.file_uri("R/test.R");
    context.open_file(&file_uri, "variable_name <- 1\n").await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("```text\nvariable_name\n```"),
        "expected hover to include the identifier name in a fenced block, got: {value}"
    );
    assert!(
        !value.contains("### Parsing"),
        "expected hover to omit parsing section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_identifier_debug_info_when_debug_enabled() {
    let mut context = setup_test_with_features(&[], &["hovering", "debug"]).await;

    let file_uri = context.file_uri("R/test_debug.R");
    context.open_file(&file_uri, "variable_name <- 1\n").await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("```text\nvariable_name\n```"),
        "expected hover to include the identifier name in a fenced block, got: {value}"
    );
    assert!(
        value.contains("### Parsing"),
        "expected hover to include a parsing section when enabled, got: {value}"
    );
    assert!(
        value.contains("- kind: `identifier`"),
        "expected hover to include node kind, got: {value}"
    );
    assert!(
        value.contains("- id: `"),
        "expected hover to include node id, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_truncates_literal_value_at_newline() {
    let mut context = setup_test_with_features(&[], &["hovering"]).await;

    let file_uri = context.file_uri("R/test_literal.R");
    context.open_file(&file_uri, "x <- r\"foo\nbar\"\n").await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 6),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("```r\n\"foo\n```"),
        "expected hover to include an RA-style fenced literal section, got: {value}"
    );
    assert!(
        value.contains("value of literal (truncated up to newline): ` foo `"),
        "expected truncated literal value in hover, got: {value}"
    );
    assert!(
        !value.contains("### Parsing"),
        "expected hover to omit parsing section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_literal_includes_debug_when_debug_enabled() {
    let mut context = setup_test_with_features(&[], &["hovering", "debug"]).await;

    let file_uri = context.file_uri("R/test_literal_debug.R");
    context.open_file(&file_uri, "x <- r\"foo\nbar\"\n").await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 6),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("### Parsing"),
        "expected hover to include a parsing section when enabled, got: {value}"
    );
    assert!(
        value.contains("- kind: `string`"),
        "expected hover to include literal node kind, got: {value}"
    );
    assert!(
        value.contains("- id: `"),
        "expected hover to include literal node id, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_keyword_information_for_if() {
    let mut context = setup_test_with_features(&[], &["hovering"]).await;

    let file_uri = context.file_uri("R/test_if.R");
    context
        .open_file(&file_uri, "if (TRUE) {\n  x <- 1\n}\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("```r\nif\n```"),
        "expected keyword hover summary block, got: {value}"
    );
    assert!(
        value.contains("Conditional branch."),
        "expected keyword description, got: {value}"
    );
    assert!(
        !value.contains("### Parsing"),
        "expected hover to omit parsing section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_keyword_includes_debug_when_debug_enabled() {
    let mut context = setup_test_with_features(&[], &["hovering", "debug"]).await;

    let file_uri = context.file_uri("R/test_if_debug.R");
    context
        .open_file(&file_uri, "if (TRUE) {\n  x <- 1\n}\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    let value = markup.value;
    assert!(
        value.contains("### Parsing"),
        "expected hover to include a parsing section when enabled, got: {value}"
    );
    assert!(
        value.contains("- kind: `if`"),
        "expected hover to include keyword node kind, got: {value}"
    );
    assert!(
        value.contains("- id: `"),
        "expected hover to include keyword node id, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_none_for_unsupported_nodes() {
    let mut context = setup_test_with_features(&[], &["hovering"]).await;

    let file_uri = context.file_uri("R/test_unsupported.R");
    context.open_file(&file_uri, "x <- 1 + 2\n").await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 7),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed");

    assert!(
        hover.is_none(),
        "expected no hover response for unsupported nodes, got: {hover:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn diagnostics_on_open() {
    let mut context = setup_test(&[]).await;

    let file_path = context.workspace_dir.join("R/test.R");
    std::fs::write(&file_path, "x <- T\ny = 1\n").expect("failed to write test file");

    let file_uri = context.file_uri("R/test.R");
    context.open_file(&file_uri, "x <- T\ny = 1\n").await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;

    let messages: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();

    assert!(
        messages.iter().any(|m| m.contains("TRUE")),
        "expected a diagnostic about T vs TRUE, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("<-")),
        "expected a diagnostic about = vs <-, got: {messages:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn no_diagnostics_for_clean_file() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/clean.R");
    context.open_file(&file_uri, "x <- 1\ny <- x + 2\n").await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;

    assert!(
        diagnostics.diagnostics.is_empty(),
        "expected no diagnostics, got: {:?}",
        diagnostics.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn diagnostics_on_syntax_error() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/broken.R");
    context.open_file(&file_uri, "f(\n").await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;

    let messages: Vec<&str> = diagnostics
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();

    assert!(
        messages.iter().any(|m| m.contains("missing closing")),
        "expected a diagnostic about missing closing delimiter, got: {messages:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn formatting() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/fmt.R");
    let unformatted = "x<-1\ny  <-  2\n";
    context.open_file(&file_uri, unformatted).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting request failed");

    let edits = edits.expect("expected formatting edits");
    assert!(!edits.is_empty(), "expected at least one edit");

    let formatted_text = &edits[0].new_text;
    assert!(
        formatted_text.contains("x <- 1"),
        "expected formatted output to contain 'x <- 1', got: {formatted_text}"
    );
    assert!(
        formatted_text.contains("y <- 2"),
        "expected formatted output to contain 'y <- 2', got: {formatted_text}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn goto_definition() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/defn.R");
    let source = "foo <- function(x) x\nbar <- foo(1)\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let result = context
        .server
        .definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(1, 7),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("definition request failed");

    let result = result.expect("expected a definition response");
    match result {
        GotoDefinitionResponse::Scalar(location) => {
            assert_eq!(location.uri, file_uri);
            assert_eq!(location.range.start.line, 0);
            assert_eq!(location.range.start.character, 0);
        }
        GotoDefinitionResponse::Array(locations) => {
            assert!(!locations.is_empty(), "expected at least one location");
            assert_eq!(locations[0].uri, file_uri);
            assert_eq!(locations[0].range.start.line, 0);
            assert_eq!(locations[0].range.start.character, 0);
        }
        GotoDefinitionResponse::Link(links) => {
            assert!(!links.is_empty(), "expected at least one link");
            assert_eq!(links[0].target_uri, file_uri);
            assert_eq!(links[0].target_range.start.line, 0);
        }
    }

    context.shutdown().await;
}

#[tokio::test]
async fn completion() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/comp.R");
    let source = "my_function <- function(x) x\nmy_f\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let result = context
        .server
        .completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(1, 4),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .await
        .expect("completion request failed");

    let result = result.expect("expected completions");
    let items = match result {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(
        labels.iter().any(|label| *label == "my_function"),
        "expected 'my_function' in completions, got: {labels:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn document_symbols() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/syms.R");
    let source = "add <- function(x, y) x + y\nmultiply <- function(a, b) a * b\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let result = context
        .server
        .document_symbol(DocumentSymbolParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document_symbol request failed");

    let result = result.expect("expected document symbols");
    match result {
        DocumentSymbolResponse::Nested(symbols) => {
            let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
            assert!(
                names.contains(&"add"),
                "expected 'add' in symbols, got: {names:?}"
            );
            assert!(
                names.contains(&"multiply"),
                "expected 'multiply' in symbols, got: {names:?}"
            );
        }
        DocumentSymbolResponse::Flat(symbols) => {
            let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
            assert!(
                names.contains(&"add"),
                "expected 'add' in symbols, got: {names:?}"
            );
            assert!(
                names.contains(&"multiply"),
                "expected 'multiply' in symbols, got: {names:?}"
            );
        }
    }

    context.shutdown().await;
}

#[tokio::test]
async fn config_indent_width() {
    let mut context = setup_test(&[("roughly.toml", "[format]\nindent-width = 4\n")]).await;

    let file_uri = context.file_uri("R/indent.R");
    let source = "f <- function(x) {\nx + 1\n}\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting request failed");

    let edits = edits.expect("expected formatting edits");
    assert!(!edits.is_empty(), "expected at least one edit");

    let formatted_text = &edits[0].new_text;
    assert!(
        formatted_text.contains("    x + 1"),
        "expected 4-space indentation in formatted output, got:\n{formatted_text}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn config_reload_on_change() {
    let mut context = setup_test(&[("roughly.toml", "[format]\nindent-width = 2\n")]).await;

    let file_uri = context.file_uri("R/reload.R");
    let source = "f <- function(x) {\nx + 1\n}\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let initial_edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("initial formatting request failed")
        .expect("expected initial formatting edits");

    assert!(
        initial_edits[0].new_text.contains("  x + 1"),
        "expected 2-space indentation before config reload, got:\n{}",
        initial_edits[0].new_text
    );

    std::fs::write(
        context.workspace_dir.join("roughly.toml"),
        "[format]\nindent-width = 4\n",
    )
    .expect("failed to update config");

    context.notify_watched_file_changed("roughly.toml", FileChangeType::CHANGED);

    let reloaded_edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting request after config reload failed")
        .expect("expected formatting edits after config reload");

    assert!(
        reloaded_edits[0].new_text.contains("    x + 1"),
        "expected 4-space indentation after config reload, got:\n{}",
        reloaded_edits[0].new_text
    );

    context.shutdown().await;
}
