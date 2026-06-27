use {
    async_lsp::{
        LanguageServer,
        concurrency::{Concurrency, ConcurrencyLayer},
        lsp_types::{
            ClientCapabilities, CompletionParams, CompletionResponse, DidChangeWatchedFilesParams,
            DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams,
            DocumentSymbolResponse, FileChangeType, FileEvent, FormattingOptions,
            GeneralClientCapabilities, GotoDefinitionParams, GotoDefinitionResponse, HoverContents,
            HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
            InitializedParams, PartialResultParams, Position, PositionEncodingKind,
            PublishDiagnosticsParams, ReferenceContext, ReferenceParams, RenameParams,
            TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Url,
            WorkDoneProgressParams, WorkspaceFolder,
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
    setup_test_inner(
        create_r_directory,
        initial_files,
        experimental_features,
        ClientCapabilities::default(),
    )
    .await
}

async fn setup_test_inner(
    create_r_directory: bool,
    initial_files: &[(&str, &str)],
    experimental_features: &[&str],
    capabilities: ClientCapabilities,
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
            capabilities,
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
    setup_test_inner(true, initial_files, &[], capabilities).await
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
        matches!(hover, Some(HoverProviderCapability::Simple(true))),
        "expected hover_provider to be enabled by default"
    );

    let references = &capabilities.references_provider;
    assert!(
        matches!(references, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected references_provider to be enabled by default"
    );

    let rename = &capabilities.rename_provider;
    assert!(
        matches!(rename, Some(async_lsp::lsp_types::OneOf::Left(true))),
        "expected rename_provider to be enabled by default"
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
async fn hover_returns_identifier_definition_without_debug_by_default() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/test.R");
    context
        .open_file(&file_uri, "variable_name <- 1\nresult <- variable_name\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(1, 12),
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
        value.contains("defined at"),
        "expected hover to include the variable definition location, got: {value}"
    );
    assert!(
        !value.contains("### Debug"),
        "expected hover to omit the debug section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_debug_section_when_debug_enabled() {
    let mut context = setup_test_with_features(&[], &["debug"]).await;

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
        value.contains("### Debug"),
        "expected hover to include a debug section when enabled, got: {value}"
    );
    assert!(
        value.contains("Assign(variable_name)"),
        "expected the debug section to include the lowered assignment, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_type_by_default() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/test_literal.R");
    context.open_file(&file_uri, "x <- \"foo\"\n").await;

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
        value.contains("character"),
        "expected hover to include the inferred type, got: {value}"
    );
    assert!(
        !value.contains("### Debug"),
        "expected hover to omit the debug section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_type_for_if() {
    let mut context = setup_test_with_features(&[], &[]).await;

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
        value.contains("NULL"),
        "expected hover to include the nullable if-expression type, got: {value}"
    );
    assert!(
        !value.contains("### Debug"),
        "expected hover to omit the debug section by default, got: {value}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_none_outside_expressions() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/test_outside.R");
    context.open_file(&file_uri, "x <- 1\n").await;

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
        "expected no hover response outside expressions, got: {hover:?}"
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
    let CompletionResponse::List(list) = result else {
        panic!("expected a CompletionList response carrying isIncomplete");
    };
    assert!(!list.is_incomplete, "small candidate set should not be marked incomplete");
    let items = list.items;

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(
        labels.iter().any(|label| *label == "my_function"),
        "expected 'my_function' in completions, got: {labels:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn rename_uses_analysis_across_files() {
    let mut context = setup_test_with_features(
        &[("R/a.R", "value <- 1L\n"), ("R/b.R", "result <- value\n")],
        &[],
    )
    .await;

    let file_uri = context.file_uri("R/a.R");
    context.open_file(&file_uri, "value <- 1L\n").await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let result = context
        .server
        .rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 1),
            },
            new_name: "renamed".into(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("rename request failed");

    let result = result.expect("expected rename edit");
    let changes = result.changes.expect("expected rename changes");

    let file_a_uri = context.file_uri("R/a.R");
    let file_b_uri = context.file_uri("R/b.R");

    let file_a_edits = changes.get(&file_a_uri).expect("missing file A edits");
    assert_eq!(file_a_edits.len(), 1);
    assert_eq!(file_a_edits[0].new_text, "renamed");
    assert_eq!(file_a_edits[0].range.start.line, 0);
    assert_eq!(file_a_edits[0].range.start.character, 0);

    let file_b_edits = changes.get(&file_b_uri).expect("missing file B edits");
    assert_eq!(file_b_edits.len(), 1);
    assert_eq!(file_b_edits[0].new_text, "renamed");
    assert_eq!(file_b_edits[0].range.start.line, 0);
    assert_eq!(file_b_edits[0].range.start.character, 10);

    context.shutdown().await;
}

#[tokio::test]
async fn references_use_analysis_across_files() {
    let mut context = setup_test_with_features(
        &[("R/a.R", "value <- 1L\n"), ("R/b.R", "result <- value\n")],
        &[],
    )
    .await;

    let file_uri = context.file_uri("R/a.R");
    context.open_file(&file_uri, "value <- 1L\n").await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let result = context
        .server
        .references(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 1),
            },
            context: ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("references request failed");

    let locations = result.expect("expected reference locations");
    let file_b_uri = context.file_uri("R/b.R");

    assert!(
        locations
            .iter()
            .any(|location| location.uri == file_uri && location.range.start.line == 0),
        "expected the declaration in file A, got: {locations:?}"
    );
    assert!(
        locations
            .iter()
            .any(|location| location.uri == file_b_uri && location.range.start.character == 10),
        "expected the cross-file use in file B, got: {locations:?}"
    );

    let without_declaration = context
        .server
        .references(ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 1),
            },
            context: ReferenceContext {
                include_declaration: false,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("references request failed")
        .expect("expected reference locations");

    assert!(
        without_declaration
            .iter()
            .all(|location| location.uri != file_uri),
        "expected the declaration to be excluded, got: {without_declaration:?}"
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

// Position-encoding negotiation. The analysis crate addresses text with UTF-8 byte columns
// (tree-sitter `Point` semantics); these tests assert that the server translates LSP positions to
// and from the negotiated encoding so non-ASCII lines still produce correct spans.

#[tokio::test]
async fn initialize_negotiates_utf16_by_default() {
    let context = setup_test(&[]).await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF16),
        "without client-offered encodings the server must advertise UTF-16"
    );
    context.shutdown().await;
}

#[tokio::test]
async fn initialize_negotiates_utf8_when_offered() {
    let context = setup_test_with_position_encodings(&[], &[PositionEncodingKind::UTF8]).await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF8),
        "the server must prefer UTF-8 when the client offers it"
    );
    context.shutdown().await;
}

#[tokio::test]
async fn hover_range_under_utf16_with_bmp_non_ascii() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/bmp.R");
    // `é` is one UTF-16 code unit but two UTF-8 bytes, so the byte column of `target` (16) differs
    // from its UTF-16 column (15).
    context
        .open_file(&file_uri, "target <- 1L\ny <- f(\"café\", target)\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(1, 17),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let range = hover.range.expect("hover should report a range");
    assert_eq!(range.start.line, 1);
    assert_eq!(
        (range.start.character, range.end.character),
        (15, 21),
        "expected the UTF-16 span of `target`, got: {range:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_range_under_utf16_with_non_bmp_emoji() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/emoji.R");
    // `🦀` is a non-BMP code point: two UTF-16 code units, four UTF-8 bytes, one scalar. So the byte
    // column of `target` (15) and its UTF-16 column (13) diverge by more than one.
    context
        .open_file(&file_uri, "target <- 1L\ny <- f(\"🦀\", target)\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(1, 14),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let range = hover.range.expect("hover should report a range");
    assert_eq!(range.start.line, 1);
    assert_eq!(
        (range.start.character, range.end.character),
        (13, 19),
        "expected the UTF-16 span of `target` after an emoji, got: {range:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_range_under_utf8_negotiation() {
    let mut context = setup_test_with_position_encodings(&[], &[PositionEncodingKind::UTF8]).await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF8),
    );

    let file_uri = context.file_uri("R/utf8.R");
    context
        .open_file(&file_uri, "target <- 1L\ny <- f(\"café\", target)\n")
        .await;

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                // Under UTF-8, characters are byte offsets, so `target` starts at byte column 16.
                position: Position::new(1, 18),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let range = hover.range.expect("hover should report a range");
    assert_eq!(
        (range.start.character, range.end.character),
        (16, 22),
        "expected the UTF-8 byte span of `target`, got: {range:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn goto_definition_range_under_utf16_with_non_ascii() {
    let mut context = setup_test_with_features(&[], &[]).await;

    let file_uri = context.file_uri("R/goto.R");
    // The identifier `caféx` is five scalars / five UTF-16 units but six UTF-8 bytes, so its
    // definition span ends at UTF-16 column 5 rather than byte column 6.
    context
        .open_file(&file_uri, "caféx <- 1L\ny <- caféx\n")
        .await;

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
        .expect("definition request failed")
        .expect("expected a definition response");

    let location = match result {
        GotoDefinitionResponse::Scalar(location) => location,
        GotoDefinitionResponse::Array(mut locations) => {
            locations.pop().expect("expected at least one location")
        }
        GotoDefinitionResponse::Link(_) => panic!("unexpected link response"),
    };

    assert_eq!(location.uri, file_uri);
    assert_eq!(location.range.start.line, 0);
    assert_eq!(location.range.start.character, 0);
    assert_eq!(
        location.range.end.character, 5,
        "expected the UTF-16 end column of `caféx`, got: {:?}",
        location.range
    );

    context.shutdown().await;
}

#[tokio::test]
async fn document_symbol_selection_range_under_utf16_with_non_ascii() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/symbol.R");
    // `café_fn` is seven scalars / seven UTF-16 units but eight UTF-8 bytes (`é` is two bytes), so
    // its selection range ends at UTF-16 column 7, not byte column 8.
    context.open_file(&file_uri, "café_fn <- function() 1\n").await;

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
        .expect("document_symbol request failed")
        .expect("expected document symbols");

    let symbols = match result {
        DocumentSymbolResponse::Nested(symbols) => symbols,
        DocumentSymbolResponse::Flat(_) => panic!("expected nested document symbols"),
    };

    let symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "café_fn")
        .expect("expected the café_fn symbol");

    assert_eq!(symbol.selection_range.start.character, 0);
    assert_eq!(
        symbol.selection_range.end.character, 7,
        "expected the UTF-16 end column of `café_fn`, got: {:?}",
        symbol.selection_range
    );

    context.shutdown().await;
}

#[tokio::test]
async fn diagnostics_range_under_utf16_with_non_bmp_emoji() {
    let mut context = setup_test(&[]).await;

    // Line layout of `"🦀"; x <- T`:
    //   scalar idx:  " =0  🦀 =1  " =2  ; =3  ' '=4  x =5  ' '=6  < =7  - =8  ' '=9  T =10
    // `🦀` is non-BMP: 2 UTF-16 code units / 4 UTF-8 bytes / 1 scalar. The `T` lint span is one
    // token, so its columns diverge: UTF-16 = 11..12, UTF-8 byte = 13..14. Under default utf-16
    // negotiation the server must report the UTF-16 columns (11, 12), not the byte columns.
    let source = "\"🦀\"; x <- T\n";
    let file_path = context.workspace_dir.join("R/emoji_diag.R");
    std::fs::write(&file_path, source).expect("failed to write test file");

    let file_uri = context.file_uri("R/emoji_diag.R");
    context.open_file(&file_uri, source).await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;

    let true_diagnostic = diagnostics
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("TRUE"))
        .unwrap_or_else(|| panic!("expected a T-vs-TRUE diagnostic, got: {:?}", diagnostics.diagnostics));

    assert_eq!(true_diagnostic.range.start.line, 0);
    assert_eq!(
        (
            true_diagnostic.range.start.character,
            true_diagnostic.range.end.character,
        ),
        (11, 12),
        "expected the UTF-16 span of `T`, got: {:?}",
        true_diagnostic.range
    );

    context.shutdown().await;
}
