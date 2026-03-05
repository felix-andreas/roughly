use {
    async_lsp::{
        LanguageServer,
        concurrency::{Concurrency, ConcurrencyLayer},
        lsp_types::{
            CompletionParams, CompletionResponse, DidOpenTextDocumentParams,
            DocumentFormattingParams, DocumentSymbolParams, DocumentSymbolResponse,
            FormattingOptions, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
            InitializedParams, PartialResultParams, Position, PublishDiagnosticsParams,
            TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Url,
            WorkDoneProgressParams, WorkspaceFolder, notification, request,
        },
        panic::{CatchUnwind, CatchUnwindLayer},
        router::Router,
    },
    std::{ops::ControlFlow, path::Path, process::Stdio, time::Duration},
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

        router.notification::<notification::PublishDiagnostics>(|state, params| {
            state
                .diagnostics_sender
                .send(params)
                .expect("diagnostics channel closed unexpectedly");
            ControlFlow::Continue(())
        });

        router.request::<request::RegisterCapability, _>(|_, _| std::future::ready(Ok(())));

        router.notification::<notification::ShowMessage>(|_, _| ControlFlow::Continue(()));

        router.event(|_, _: Stop| ControlFlow::Break(Ok(())));

        ServiceBuilder::new()
            .layer(CatchUnwindLayer::default())
            .layer(ConcurrencyLayer::default())
            .service(router)
    })
}

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
}

async fn setup_test(workspace_dir: &Path) -> TestContext {
    let (diagnostics_sender, diagnostics_receiver) = mpsc::unbounded_channel();
    let (mainloop, mut server) = build_test_client(diagnostics_sender);

    let mut child = spawn_server(workspace_dir);
    let stdout = child.stdout.take().expect("missing stdout").compat();
    let stdin = child.stdin.take().expect("missing stdin").compat_write();

    let mainloop_handle = tokio::spawn(async move {
        if let Err(error) = mainloop.run_buffered(stdout, stdin).await {
            eprintln!("mainloop error: {error}");
        }
        drop(child);
    });

    let root_uri = Url::from_file_path(workspace_dir).expect("invalid workspace path");

    let _init_result = server
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
    }
}

impl TestContext {
    async fn shutdown(mut self) {
        self.server.shutdown(()).await.expect("shutdown failed");
        self.server.exit(()).expect("exit failed");
        self.server.emit(Stop).expect("emit Stop failed");
        let _ = tokio::time::timeout(Duration::from_secs(3), self.mainloop_handle).await;
    }

    fn file_uri(workspace_dir: &Path, relative_path: &str) -> Url {
        Url::from_file_path(workspace_dir.join(relative_path)).expect("invalid file path")
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
}

#[tokio::test]
async fn initialize_reports_capabilities() {
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    std::fs::create_dir_all(workspace_dir.path().join("R")).expect("failed to create R directory");

    let (diagnostics_sender, _diagnostics_receiver) = mpsc::unbounded_channel();
    let (mainloop, mut server) = build_test_client(diagnostics_sender);

    let mut child = spawn_server(workspace_dir.path());
    let stdout = child.stdout.take().expect("missing stdout").compat();
    let stdin = child.stdin.take().expect("missing stdin").compat_write();

    let mainloop_handle = tokio::spawn(async move {
        let _ = mainloop.run_buffered(stdout, stdin).await;
        drop(child);
    });

    let root_uri = Url::from_file_path(workspace_dir.path()).expect("invalid path");

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

    let capabilities = init_result.capabilities;

    let sync = capabilities
        .text_document_sync
        .expect("missing text_document_sync");
    let sync_options = match sync {
        async_lsp::lsp_types::TextDocumentSyncCapability::Options(options) => options,
        _ => panic!("expected TextDocumentSyncOptions"),
    };
    assert_eq!(
        sync_options.change,
        Some(async_lsp::lsp_types::TextDocumentSyncKind::INCREMENTAL)
    );

    let formatting = capabilities.document_formatting_provider;
    assert!(
        matches!(formatting, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected document_formatting_provider to be true"
    );

    let definition = capabilities.definition_provider;
    assert!(
        matches!(definition, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected definition_provider to be true"
    );

    let completion = capabilities
        .completion_provider
        .expect("missing completion_provider");
    let trigger_chars = completion
        .trigger_characters
        .expect("missing trigger_characters");
    assert!(trigger_chars.contains(&"$".into()));
    assert!(trigger_chars.contains(&"@".into()));
    assert!(trigger_chars.contains(&":".into()));

    let doc_symbol = capabilities.document_symbol_provider;
    assert!(
        matches!(doc_symbol, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected document_symbol_provider to be true"
    );

    server.shutdown(()).await.expect("shutdown failed");
    server.exit(()).expect("exit failed");
    server.emit(Stop).expect("emit Stop failed");
    let _ = tokio::time::timeout(Duration::from_secs(3), mainloop_handle).await;
}

#[tokio::test]
async fn diagnostics_on_open() {
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let file_path = r_dir.join("test.R");
    std::fs::write(&file_path, "x <- T\ny = 1\n").expect("failed to write test file");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/test.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/clean.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/broken.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/fmt.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/defn.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/comp.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/syms.R");
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
    let workspace_dir = tempfile::tempdir().expect("failed to create temp dir");
    let r_dir = workspace_dir.path().join("R");
    std::fs::create_dir_all(&r_dir).expect("failed to create R directory");

    std::fs::write(
        workspace_dir.path().join("roughly.toml"),
        "[format]\nindent-width = 4\n",
    )
    .expect("failed to write config");

    let mut context = setup_test(workspace_dir.path()).await;

    let file_uri = TestContext::file_uri(workspace_dir.path(), "R/indent.R");
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
