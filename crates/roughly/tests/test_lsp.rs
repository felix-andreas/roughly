use {
    async_lsp::{
        LanguageServer,
        concurrency::{Concurrency, ConcurrencyLayer},
        lsp_types::{
            ClientCapabilities, CompletionParams, CompletionResponse, DiagnosticClientCapabilities,
            DiagnosticServerCapabilities, DiagnosticWorkspaceClientCapabilities,
            DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
            DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentDiagnosticParams,
            DocumentDiagnosticReport, DocumentDiagnosticReportResult, DocumentFormattingParams,
            DocumentHighlightParams, DocumentSymbolParams, DocumentSymbolResponse, FileChangeType,
            FileEvent, FoldingRangeParams, FormattingOptions, GeneralClientCapabilities,
            GotoDefinitionParams, GotoDefinitionResponse, HoverContents, HoverParams,
            HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
            InlayHintParams, MessageType, ParameterInformationSettings, ParameterLabel,
            PartialResultParams, Position, PositionEncodingKind, PublishDiagnosticsParams, Range,
            ReferenceContext, ReferenceParams, RenameParams, SemanticTokensParams,
            SemanticTokensResult, ShowMessageParams, SignatureHelpClientCapabilities,
            SignatureHelpParams, SignatureInformationSettings, TextDocumentClientCapabilities,
            TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
            TextDocumentPositionParams, Url, VersionedTextDocumentIdentifier,
            WorkDoneProgressParams, WorkspaceClientCapabilities, WorkspaceFolder,
            notification::{PublishDiagnostics, ShowMessage},
            request::{RegisterCapability, WorkspaceDiagnosticRefresh},
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

fn spawn_server_with_experimental_features(
    server_cwd: &Path,
    experimental_features: &[&str],
) -> tokio::process::Child {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_roughly"));
    command.arg("server");
    if !experimental_features.is_empty() {
        command.arg("--experimental-features");
        command.arg(experimental_features.join(" "));
    }
    command
        .current_dir(server_cwd)
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
    while receiver.try_recv().is_ok() {}
}

const TIMEOUT: Duration = Duration::from_secs(5);

struct TestContext {
    server: async_lsp::ServerSocket,
    diagnostics_receiver: mpsc::UnboundedReceiver<PublishDiagnosticsParams>,
    refresh_receiver: mpsc::UnboundedReceiver<()>,
    messages_receiver: mpsc::UnboundedReceiver<ShowMessageParams>,
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
    // The server process is spawned with the temp ROOT as its working directory while the client
    // announces the `workspace` subdirectory as the workspace root, so every test exercises the
    // server deriving its root from the initialize params rather than from the process cwd.
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

    let mut child = spawn_server_with_experimental_features(&server_cwd, experimental_features);
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
        refresh_receiver,
        messages_receiver,
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

async fn setup_test_with_pull_diagnostics(initial_files: &[(&str, &str)]) -> TestContext {
    let capabilities = ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            diagnostic: Some(DiagnosticClientCapabilities::default()),
            ..TextDocumentClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    setup_test_inner(true, initial_files, &[], capabilities).await
}

async fn setup_test_with_parameter_label_offsets(initial_files: &[(&str, &str)]) -> TestContext {
    let capabilities = ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            signature_help: Some(SignatureHelpClientCapabilities {
                signature_information: Some(SignatureInformationSettings {
                    parameter_information: Some(ParameterInformationSettings {
                        label_offset_support: Some(true),
                    }),
                    ..SignatureInformationSettings::default()
                }),
                ..SignatureHelpClientCapabilities::default()
            }),
            ..TextDocumentClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    };
    setup_test_inner(true, initial_files, &[], capabilities).await
}

async fn setup_test_with_pull_and_refresh(initial_files: &[(&str, &str)]) -> TestContext {
    let capabilities = ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            diagnostic: Some(DiagnosticClientCapabilities::default()),
            ..TextDocumentClientCapabilities::default()
        }),
        workspace: Some(WorkspaceClientCapabilities {
            diagnostic: Some(DiagnosticWorkspaceClientCapabilities {
                refresh_support: Some(true),
            }),
            ..WorkspaceClientCapabilities::default()
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

    async fn document_diagnostic(
        &mut self,
        uri: &Url,
        previous_result_id: Option<String>,
    ) -> DocumentDiagnosticReportResult {
        self.server
            .document_diagnostic(DocumentDiagnosticParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                identifier: Some("roughly".into()),
                previous_result_id,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect("document_diagnostic request failed")
    }

    // Replaces `range` of the open document with `text` (incremental sync) at `version`.
    fn change_file(&mut self, uri: &Url, version: i32, range: Range, text: &str) {
        self.server
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(range),
                    range_length: None,
                    text: text.into(),
                }],
            })
            .expect("did_change failed");
    }

    // Replaces the whole document with `text` (a range-less change): the full-text-sync fallback
    // clients may legally use even though the server negotiated incremental sync.
    fn replace_file_full(&mut self, uri: &Url, version: i32, text: &str) {
        self.server
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.into(),
                }],
            })
            .expect("did_change failed");
    }

    fn save_file(&mut self, uri: &Url) {
        self.server
            .did_save(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                text: None,
            })
            .expect("did_save failed");
    }

    fn close_file(&mut self, uri: &Url) {
        self.server
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            })
            .expect("did_close failed");
    }

    // Waits briefly for a `workspace/diagnostic/refresh` request, returning whether one arrived.
    async fn received_refresh(&mut self) -> bool {
        tokio::time::timeout(Duration::from_secs(2), self.refresh_receiver.recv())
            .await
            .map(|received| received.is_some())
            .unwrap_or(false)
    }

    // Waits briefly for a `window/showMessage` notification, returning it when one arrives.
    async fn received_message(&mut self) -> Option<ShowMessageParams> {
        tokio::time::timeout(Duration::from_secs(2), self.messages_receiver.recv())
            .await
            .ok()
            .flatten()
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
async fn requests_for_unopened_non_file_uris_answer_gracefully() {
    // Editors send requests for non-`file:` buffers (`untitled:`, `vscode-vfs:`, ...). An open one is
    // served as a standalone script document (see `untitled_document_is_served_as_a_standalone_script`);
    // one the server never saw opened must still be answered — with an error response or an empty
    // result — rather than panicking the worker thread, which would terminate the server process.
    let mut context = setup_test(&[("R/main.R", "x <- 1L")]).await;
    let untitled = Url::parse("untitled:Untitled-9").expect("untitled uri should parse");

    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: untitled.clone(),
                },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await;
    assert!(
        matches!(hover, Err(_) | Ok(None)),
        "hover on an unopened non-file uri must be answered, got: {hover:?}"
    );

    let report = context.document_diagnostic(&untitled, None).await;
    assert!(
        matches!(
            report,
            DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(_))
        ),
        "a pull diagnostic on an unopened non-file uri should return an empty full report"
    );

    // The server is still alive and serving regular documents.
    let file_uri = context.file_uri("R/main.R");
    context.open_file(&file_uri, "x <- 1L").await;
    recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;

    context.shutdown().await;
}

#[tokio::test]
async fn untitled_document_is_served_as_a_standalone_script() {
    // A non-`file:` URI (a VS Code untitled buffer with the R language mode is the realistic case)
    // cannot join package analysis, but the server serves it as a standalone script document:
    // diagnostics, hover, and incremental edits work like for any open file, and the untitled
    // traffic must never take down analysis of regular documents.
    let mut context = setup_test(&[]).await;
    let untitled = Url::parse("untitled:Untitled-1").expect("untitled uri should parse");

    context
        .open_file(&untitled, "answer <- 42L\nfinal = answer\n")
        .await;

    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &untitled, TIMEOUT).await;
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("<-")),
        "expected the `=` lint on the untitled buffer, got: {:?}",
        diagnostics.diagnostics
    );

    // IDE features answer inside the untitled buffer: hover over the `answer` reference.
    let hover = context
        .server
        .hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: untitled.clone(),
                },
                position: Position::new(1, 10),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover on an untitled document failed")
        .expect("hover response missing");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("expected markup hover contents");
    };
    assert!(
        markup.value.contains("defined at"),
        "expected hover in the untitled buffer to resolve `answer`, got: {}",
        markup.value
    );

    // Outgoing edits map back to the client's URI, never the internal synthetic key: renaming
    // `answer` yields edits keyed by the untitled URI.
    let rename = context
        .server
        .rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: untitled.clone(),
                },
                position: Position::new(0, 1),
            },
            new_name: "result".into(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("rename in an untitled document failed")
        .expect("expected a rename edit");
    let changes = rename.changes.expect("expected rename changes");
    assert!(
        changes.keys().all(|uri| uri == &untitled),
        "rename edits must be keyed by the untitled URI, got: {:?}",
        changes.keys().collect::<Vec<_>>()
    );
    let untitled_edits = changes
        .get(&untitled)
        .expect("expected edits for the untitled document");
    assert_eq!(
        untitled_edits.len(),
        2,
        "expected both occurrences of `answer` to be renamed, got: {untitled_edits:?}"
    );

    // Incremental edits apply to the untitled buffer: fix the `=` and the lint disappears.
    context.change_file(
        &untitled,
        1,
        Range::new(Position::new(1, 6), Position::new(1, 7)),
        "<-",
    );
    let after_fix = recv_diagnostics(&mut context.diagnostics_receiver, &untitled, TIMEOUT).await;
    assert!(
        after_fix.diagnostics.is_empty(),
        "expected the fixed untitled buffer to be clean, got: {:?}",
        after_fix.diagnostics
    );

    // Closing the untitled buffer unregisters it, and regular documents keep working.
    context.close_file(&untitled);
    let file_uri = context.file_uri("R/after_untitled.R");
    context.open_file(&file_uri, "x <- T\n").await;
    let regular = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        regular
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "expected regular documents to keep working after untitled traffic, got: {:?}",
        regular.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn full_text_did_change_replaces_the_document() {
    // A `didChange` without a range is a whole-document replacement. Clients may legally fall back
    // to full-text sync even though the server negotiated incremental sync, so it must apply
    // instead of killing the server.
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/full_sync.R");
    context.open_file(&file_uri, "x <- T\n").await;

    let initial = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        initial
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "expected the T-vs-TRUE lint before the replacement, got: {:?}",
        initial.diagnostics
    );

    context.replace_file_full(&file_uri, 1, "y = 1\n");

    let after = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        after
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("<-")),
        "expected the `=` lint from the replacement text, got: {:?}",
        after.diagnostics
    );
    assert!(
        !after
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "the old text must be fully replaced, not merged, got: {:?}",
        after.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn did_close_survives_a_failing_disk_reread() {
    // Closing a package document reverts it to its on-disk text, but that read can race a
    // concurrent delete or rename. Simulate a close-time read that fails although the path exists
    // (the path is a directory), which must degrade like a deletion instead of killing the server.
    let mut context = setup_test(&[]).await;
    std::fs::create_dir_all(context.workspace_dir.join("R/casualty.R"))
        .expect("failed to create directory posing as a source file");

    let casualty_uri = context.file_uri("R/casualty.R");
    context.open_file(&casualty_uri, "x <- 1\n").await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;
    context.close_file(&casualty_uri);

    // The server treated the unreadable file as deleted and keeps serving.
    let file_uri = context.file_uri("R/still_alive.R");
    context.open_file(&file_uri, "x <- T\n").await;
    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "expected the server to survive the failing close-time re-read, got: {:?}",
        diagnostics.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn hover_returns_debug_section_when_debug_enabled() {
    let mut context = setup_test(&[("roughly.toml", "debug = true\n")]).await;

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
    assert!(
        !list.is_incomplete,
        "small candidate set should not be marked incomplete"
    );
    let items = list.items;

    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    assert!(
        labels.contains(&"my_function"),
        "expected 'my_function' in completions, got: {labels:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn inlay_hints_respect_requested_viewport() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/hints.R");
    let source = "count <- 1L\nlabel <- \"hello\"\nratio <- 2L\n";
    context.open_file(&file_uri, source).await;

    drain_diagnostics(&mut context.diagnostics_receiver).await;

    // Request only the middle line; hints on the surrounding lines must not be returned.
    let hints = context
        .server
        .inlay_hint(InlayHintParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            range: Range::new(Position::new(1, 0), Position::new(1, 16)),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("inlay hint request failed")
        .expect("expected inlay hints");

    let lines: Vec<u32> = hints.iter().map(|hint| hint.position.line).collect();
    assert_eq!(
        lines,
        vec![1],
        "only the in-viewport hint should be returned"
    );

    context.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn folding_ranges_cover_braces_and_comment_runs() {
    let source = "#: fn(x: integer)\n#: -> integer\nf <- function(x) {\n  x\n}\n";
    let mut context = setup_test_with_features(&[("R/a.R", source)], &[]).await;

    let file_uri = context.file_uri("R/a.R");
    context.open_file(&file_uri, source).await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let ranges = context
        .server
        .folding_range(FoldingRangeParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("folding range failed")
        .expect("expected folding ranges");

    assert!(
        ranges
            .iter()
            .any(|range| range.start_line == 0 && range.end_line == 1),
        "the two-line annotation block should fold: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|range| range.start_line == 2 && range.end_line == 3),
        "the function body braces should fold: {ranges:?}"
    );

    context.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn document_highlight_marks_all_occurrences_in_the_file() {
    let mut context =
        setup_test_with_features(&[("R/a.R", "value <- 1L\nresult <- value + value\n")], &[]).await;

    let file_uri = context.file_uri("R/a.R");
    context
        .open_file(&file_uri, "value <- 1L\nresult <- value + value\n")
        .await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let highlights = context
        .server
        .document_highlight(DocumentHighlightParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 1),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("document highlight failed")
        .expect("expected highlights");

    assert_eq!(
        highlights.len(),
        3,
        "declaration + two reads: {highlights:?}"
    );

    context.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_rejects_a_non_syntactic_name() {
    let mut context = setup_test_with_features(&[("R/a.R", "value <- 1L\n")], &[]).await;

    let file_uri = context.file_uri("R/a.R");
    context.open_file(&file_uri, "value <- 1L\n").await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    for bad_name in ["my var", "1st", "function", ""] {
        let result = context
            .server
            .rename(RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position: Position::new(0, 1),
                },
                new_name: bad_name.into(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await;
        assert!(
            result.is_err(),
            "renaming to {bad_name:?} must be rejected, got: {result:?}"
        );
    }

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
    context
        .open_file(&file_uri, "café_fn <- function() 1\n")
        .await;

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
        .unwrap_or_else(|| {
            panic!(
                "expected a T-vs-TRUE diagnostic, got: {:?}",
                diagnostics.diagnostics
            )
        });

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

#[tokio::test]
async fn hover_range_under_utf8_with_non_bmp_emoji() {
    let mut context = setup_test_with_position_encodings(&[], &[PositionEncodingKind::UTF8]).await;
    assert_eq!(
        context.init_result.capabilities.position_encoding,
        Some(PositionEncodingKind::UTF8),
    );

    let file_uri = context.file_uri("R/utf8_emoji.R");
    // Under UTF-8 negotiation, columns ARE byte offsets, so the non-BMP `🦀` counts as 4. Line
    // `y <- f("🦀", target)`, scalar idx of `target` start = 12; bytes before it: chars 0..7 = 8
    // bytes, `🦀` = 4 bytes, `"` `,` ` ` = 3 bytes -> `target` byte span = 15..21 (vs UTF-16 13..19).
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
                // byte column 17 is inside `target` (bytes 15..21)
                position: Position::new(1, 17),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("hover request failed")
        .expect("hover response missing");

    let range = hover.range.expect("hover should report a range");
    assert_eq!(
        (range.start.character, range.end.character),
        (15, 21),
        "expected the UTF-8 byte span of `target` after an emoji, got: {range:?}"
    );

    context.shutdown().await;
}

// S6: out-of-bounds / stale position safety on every IDE entry point. The document has an empty
// line 1, so `Position::new(1, 50)` is a character past the end of a (zero-length) line, and
// `Position::new(50, 0)` is a line past the last line. The position-encoding conversion
// (`to_internal_position`/`to_internal_range`) clamps both coordinates to the document — the line
// to the last line, the character to that line's length — so nothing past-EOF ever reaches
// analysis; the IDE lookups then answer the clamped, in-bounds position (`None`/empty wherever it
// hits no analysis target).

const OOB_DOC: &str = "alpha <- 1\n\nbeta <- 2\n";

fn oob_positions() -> [Position; 2] {
    // character past end of the empty line 1, and a line past the last line
    [Position::new(1, 50), Position::new(50, 0)]
}

#[tokio::test]
async fn hover_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_hover.R");
    context.open_file(&file_uri, OOB_DOC).await;

    for position in oob_positions() {
        let hover = context
            .server
            .hover(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect("hover request must not panic the server");
        assert!(
            hover.is_none(),
            "OOB hover should be None at {position:?}, got: {hover:?}"
        );
    }

    context.shutdown().await;
}

#[tokio::test]
async fn goto_definition_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_goto.R");
    context.open_file(&file_uri, OOB_DOC).await;

    for position in oob_positions() {
        let result = context
            .server
            .definition(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect("definition request must not panic the server");
        assert!(
            result.is_none(),
            "OOB goto should be None at {position:?}, got: {result:?}"
        );
    }

    context.shutdown().await;
}

#[tokio::test]
async fn references_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_refs.R");
    context.open_file(&file_uri, OOB_DOC).await;

    for position in oob_positions() {
        let result = context
            .server
            .references(ReferenceParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                context: ReferenceContext {
                    include_declaration: true,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect("references request must not panic the server");
        assert!(
            result.as_ref().is_none_or(|locations| locations.is_empty()),
            "OOB references should be None/empty at {position:?}, got: {result:?}"
        );
    }

    context.shutdown().await;
}

#[tokio::test]
async fn rename_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_rename.R");
    context.open_file(&file_uri, OOB_DOC).await;

    for position in oob_positions() {
        let result = context
            .server
            .rename(RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                new_name: "renamed".into(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect("rename request must not panic the server");
        assert!(
            result.is_none(),
            "OOB rename must produce no edits at {position:?}, got: {result:?}"
        );
    }

    context.shutdown().await;
}

#[tokio::test]
async fn completion_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_completion.R");
    context.open_file(&file_uri, OOB_DOC).await;

    // Both coordinates clamp to the document (a line past the last line to the last line, a
    // character past the line end to the line end), which are valid completion positions, so
    // completion may legitimately return keyword/global candidates; the contract here is that the
    // clamped requests do not panic.
    for position in oob_positions() {
        let _completions = context
            .server
            .completion(CompletionParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: None,
            })
            .await
            .expect("completion request must not panic the server");
    }

    context.shutdown().await;
}

// The signature label must be the generalized scheme (internal inference variables never reach the
// client), and each parameter must arrive as `[start, end)` offsets that slice exactly that
// parameter out of the label when the client advertises label-offset support.
#[tokio::test]
async fn signature_help_sends_generalized_label_with_parameter_offsets() {
    let mut context = setup_test_with_parameter_label_offsets(&[]).await;
    let file_uri = context.file_uri("R/sig.R");
    context.open_file(&file_uri, "result <- lapply()\n").await;

    let help = context
        .server
        .signature_help(SignatureHelpParams {
            context: None,
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 17),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("signature_help request failed")
        .expect("signature help expected inside the call");

    let signature = &help.signatures[0];
    assert_eq!(
        signature.label,
        "<T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]"
    );
    let parameters = signature
        .parameters
        .as_ref()
        .expect("signature parameters expected");
    let parameter_texts = parameters
        .iter()
        .map(|parameter| match parameter.label {
            ParameterLabel::LabelOffsets([start, end]) => signature
                .label
                .get(start as usize..end as usize)
                .expect("parameter offsets must slice the label"),
            ParameterLabel::Simple(_) => {
                panic!("offset-capable client must receive label offsets")
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(parameter_texts, ["x: list[T]", "f: fn(T) -> U"]);
    assert_eq!(help.active_parameter, Some(0));

    context.shutdown().await;
}

// Without the label-offset capability each parameter falls back to its exact substring of the
// label, which substring-matching clients locate themselves.
#[tokio::test]
async fn signature_help_falls_back_to_substring_parameter_labels() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/sig.R");
    context.open_file(&file_uri, "result <- lapply()\n").await;

    let help = context
        .server
        .signature_help(SignatureHelpParams {
            context: None,
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: file_uri.clone(),
                },
                position: Position::new(0, 17),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("signature_help request failed")
        .expect("signature help expected inside the call");

    let signature = &help.signatures[0];
    let parameters = signature
        .parameters
        .as_ref()
        .expect("signature parameters expected");
    let parameter_texts = parameters
        .iter()
        .map(|parameter| match &parameter.label {
            ParameterLabel::Simple(text) => text.as_str(),
            ParameterLabel::LabelOffsets(_) => {
                panic!("client without offset support must receive substring labels")
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(parameter_texts, ["x: list[T]", "f: fn(T) -> U"]);

    context.shutdown().await;
}

#[tokio::test]
async fn signature_help_out_of_bounds_position_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_sighelp.R");
    context.open_file(&file_uri, OOB_DOC).await;

    for position in oob_positions() {
        let result = context
            .server
            .signature_help(SignatureHelpParams {
                context: None,
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: file_uri.clone(),
                    },
                    position,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect("signature_help request must not panic the server");
        assert!(
            result.is_none(),
            "OOB signature help should be None at {position:?}, got: {result:?}"
        );
    }

    context.shutdown().await;
}

// Pull diagnostics (`textDocument/diagnostic`, LSP 3.17). The pull result must equal what the push
// path would send for the same state, and is additive: push remains the default for clients that do
// not advertise pull support.

fn full_report_messages(result: DocumentDiagnosticReportResult) -> (Vec<String>, Option<String>) {
    match result {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(full)) => {
            let report = full.full_document_diagnostic_report;
            let messages = report
                .items
                .iter()
                .map(|item| item.message.clone())
                .collect();
            (messages, report.result_id)
        }
        other => panic!("expected a full diagnostic report, got: {other:?}"),
    }
}

#[tokio::test]
async fn initialize_advertises_diagnostic_provider() {
    let context = setup_test(&[]).await;

    let provider = context
        .init_result
        .capabilities
        .diagnostic_provider
        .as_ref()
        .expect("expected a diagnostic_provider capability");
    let DiagnosticServerCapabilities::Options(options) = provider else {
        panic!("expected DiagnosticServerCapabilities::Options, got: {provider:?}");
    };
    assert_eq!(options.identifier.as_deref(), Some("roughly"));
    assert!(
        options.inter_file_dependencies,
        "package-visible edits move diagnostics in dependent files"
    );
    assert!(
        !options.workspace_diagnostics,
        "workspace/diagnostic is not implemented yet"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_report_known_diagnostics() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/pull.R");
    context.open_file(&file_uri, "x <- T\ny = 1\n").await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let (messages, result_id) =
        full_report_messages(context.document_diagnostic(&file_uri, None).await);

    assert!(
        messages.iter().any(|message| message.contains("TRUE")),
        "expected a T-vs-TRUE diagnostic in the pull report, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|message| message.contains("<-")),
        "expected an = vs <- diagnostic in the pull report, got: {messages:?}"
    );
    assert!(
        result_id.is_some(),
        "a non-empty report must carry a result id"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_empty_for_clean_file() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/pull_clean.R");
    context.open_file(&file_uri, "x <- 1\ny <- x + 2\n").await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let (messages, _) = full_report_messages(context.document_diagnostic(&file_uri, None).await);

    assert!(
        messages.is_empty(),
        "expected an empty full report for a clean file, got: {messages:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_untracked_document_is_empty() {
    let mut context = setup_test(&[]).await;

    // A pull can arrive for a document the server never opened or preloaded; it must answer with an
    // empty full report rather than panicking.
    let file_uri = context.file_uri("R/never_opened.R");
    let (messages, _) = full_report_messages(context.document_diagnostic(&file_uri, None).await);

    assert!(
        messages.is_empty(),
        "expected an empty report for an untracked document, got: {messages:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_unchanged_on_repeat() {
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/pull_unchanged.R");
    context.open_file(&file_uri, "x <- T\n").await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let (_, result_id) = full_report_messages(context.document_diagnostic(&file_uri, None).await);
    let result_id = result_id.expect("first pull must carry a result id");

    let repeat = context
        .document_diagnostic(&file_uri, Some(result_id.clone()))
        .await;
    match repeat {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(unchanged)) => {
            assert_eq!(
                unchanged.unchanged_document_diagnostic_report.result_id, result_id,
                "an unchanged report echoes the still-current result id"
            );
        }
        other => panic!("expected an unchanged report on repeat pull, got: {other:?}"),
    }

    context.shutdown().await;
}

#[tokio::test]
async fn pull_diagnostics_match_pushed_across_files() {
    // A multi-file workspace: each file is pushed on open (default client has no pull support), and a
    // subsequent pull for the same state must return the identical diagnostics.
    let mut context = setup_test(&[]).await;

    let file_a_uri = context.file_uri("R/match_a.R");
    let file_b_uri = context.file_uri("R/match_b.R");
    context.open_file(&file_a_uri, "x <- T\n").await;
    context.open_file(&file_b_uri, "y = 1\n").await;

    let pushed_a = recv_diagnostics(&mut context.diagnostics_receiver, &file_a_uri, TIMEOUT).await;
    let pushed_b = recv_diagnostics(&mut context.diagnostics_receiver, &file_b_uri, TIMEOUT).await;
    let pushed_a: Vec<String> = pushed_a
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    let pushed_b: Vec<String> = pushed_b
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();

    let (pulled_a, _) = full_report_messages(context.document_diagnostic(&file_a_uri, None).await);
    let (pulled_b, _) = full_report_messages(context.document_diagnostic(&file_b_uri, None).await);

    assert_eq!(
        pulled_a, pushed_a,
        "pulled file A diagnostics must equal the pushed ones"
    );
    assert_eq!(
        pulled_b, pushed_b,
        "pulled file B diagnostics must equal the pushed ones"
    );
    assert!(
        !pulled_a.is_empty() && !pulled_b.is_empty(),
        "both files should report diagnostics"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_capable_client_suppresses_push() {
    let mut context = setup_test_with_pull_diagnostics(&[]).await;

    let file_uri = context.file_uri("R/pull_only.R");
    context.open_file(&file_uri, "x <- T\n").await;

    // The client advertised pull support, so the server must not push. Give the server time to
    // process did_open, then assert nothing landed on the push channel.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        context.diagnostics_receiver.try_recv().is_err(),
        "a pull-capable client must not receive pushed diagnostics"
    );

    let (messages, _) = full_report_messages(context.document_diagnostic(&file_uri, None).await);
    assert!(
        messages.iter().any(|message| message.contains("TRUE")),
        "the pull report must still carry the diagnostic, got: {messages:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn cancelled_pull_diagnostics_is_retryable_never_authoritative_empty() {
    // An edit racing a pull must never be answered with an empty full report — the report is
    // authoritative (the client caches it until the next pull). Whether the edit lands before the
    // read starts (the read then correctly answers the pre-edit state) or mid-read (the engine
    // observes the cancellation token at its next checkpoint and the server answers
    // `ServerCancelled`) is timing- and checkpoint-dependent, so both legal outcomes are asserted
    // strictly; the empty-report bug is caught either way. The exact wire shape of the
    // cancellation answer is pinned deterministically by a unit test in `server.rs`.
    let mut context = setup_test_with_pull_diagnostics(&[]).await;
    let file_uri = context.file_uri("R/cancel_race.R");
    // A pull client gets no push on open, so the first pull computes cold over a document large
    // enough to leave a real race window; every line carries known lints.
    let source = "x = T\n".repeat(2000);
    context.open_file(&file_uri, &source).await;

    let mut racing_server = context.server.clone();
    let pull = racing_server.document_diagnostic(DocumentDiagnosticParams {
        text_document: TextDocumentIdentifier {
            uri: file_uri.clone(),
        },
        identifier: Some("roughly".into()),
        previous_result_id: None,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    context.change_file(
        &file_uri,
        1,
        Range::new(Position::new(0, 0), Position::new(0, 1)),
        "y",
    );

    match pull.await {
        Ok(result) => {
            let (messages, _) = full_report_messages(result);
            assert!(
                !messages.is_empty(),
                "a completed pull for this document must report its diagnostics; an empty full \
                 report means a cancelled read was served as authoritative"
            );
        }
        Err(async_lsp::Error::Response(response)) => {
            assert_eq!(
                response.code,
                async_lsp::ErrorCode::SERVER_CANCELLED,
                "a cancelled pull must answer ServerCancelled, got: {response:?}"
            );
            let data = response
                .data
                .expect("a cancelled pull must carry DiagnosticServerCancellationData");
            assert_eq!(
                data.get("retriggerRequest"),
                Some(&serde_json::Value::Bool(true)),
                "the cancellation data must ask the client to retrigger, got: {data:?}"
            );
        }
        Err(other) => panic!("unexpected pull failure: {other:?}"),
    }

    // Either way the client can retry and get the (post-edit) authoritative report.
    let (messages, _) = full_report_messages(context.document_diagnostic(&file_uri, None).await);
    assert!(
        !messages.is_empty(),
        "the retried pull must report the document's diagnostics"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn inlay_hint_on_document_is_safe() {
    let mut context = setup_test(&[]).await;
    let file_uri = context.file_uri("R/oob_inlay.R");
    context.open_file(&file_uri, OOB_DOC).await;

    // `inlay_hint` takes no position (it scans the whole document), so there is no OOB position to
    // pass; this asserts the entry point itself resolves without panicking on this document.
    let hints = context
        .server
        .inlay_hint(InlayHintParams {
            text_document: TextDocumentIdentifier {
                uri: file_uri.clone(),
            },
            range: Range::new(Position::new(0, 0), Position::new(50, 50)),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("inlay_hint request must not panic the server");
    assert!(
        hints.is_some(),
        "inlay_hint should return a (possibly empty) list"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn dependency_affecting_save_requests_diagnostic_refresh() {
    // `R/a.R` defines the global `x`; `R/b.R` references it and resolves cleanly.
    let mut context =
        setup_test_with_pull_and_refresh(&[("R/a.R", "x <- 1L\n"), ("R/b.R", "y <- x\n")]).await;
    let a_uri = context.file_uri("R/a.R");
    let b_uri = context.file_uri("R/b.R");

    // Baseline pull (runs the full check), so the later save is measured against a known state.
    let (baseline, _) = full_report_messages(context.document_diagnostic(&b_uri, None).await);
    assert!(
        baseline.is_empty(),
        "`y <- x` should resolve while `x` is defined, got: {baseline:?}"
    );

    context.open_file(&a_uri, "x <- 1L\n").await;
    // Rename the global `x` to `z` (replace the first character), so `R/b.R`'s reference to `x`
    // becomes unresolved — a package-visible change in a *different* document than the saved one.
    context.change_file(
        &a_uri,
        1,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 1,
            },
        },
        "z",
    );
    context.save_file(&a_uri);

    assert!(
        context.received_refresh().await,
        "a save that moves a dependent file's diagnostics must request workspace/diagnostic/refresh"
    );

    // The dependent's moved diagnostic is observable on the next pull.
    let (after, _) = full_report_messages(context.document_diagnostic(&b_uri, None).await);
    assert!(
        after.iter().any(|message| message.contains("resolve")),
        "after renaming `x`, `R/b.R` should report an unresolved reference, got: {after:?}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn pull_client_without_refresh_support_gets_no_refresh() {
    // A pull client that does not advertise `workspace/diagnostic/refresh` must never receive a
    // refresh request — the server respects the negotiated capability.
    let mut context = setup_test_with_pull_diagnostics(&[("R/solo.R", "x <- 1L\n")]).await;
    let solo_uri = context.file_uri("R/solo.R");

    context.open_file(&solo_uri, "x <- 1L\n").await;
    context.change_file(
        &solo_uri,
        1,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 1,
            },
        },
        "z",
    );
    context.save_file(&solo_uri);

    assert!(
        !context.received_refresh().await,
        "a client without refresh support must not receive a refresh request"
    );

    context.shutdown().await;
}

// Config subsystem: the workspace root comes from the client, `roughly.toml` is discovered by
// nearest-ancestor search from that root, a malformed config never takes the server down, and a
// config reload applies to diagnostics immediately.

async fn formatted_text(context: &mut TestContext, uri: &Url) -> String {
    let edits = context
        .server
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..FormattingOptions::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .expect("formatting request failed")
        .expect("expected formatting edits");
    edits[0].new_text.clone()
}

#[tokio::test]
async fn workspace_root_comes_from_the_client_not_the_process_cwd() {
    // A decoy config in the server process's working directory (the temp root, one level above the
    // workspace) and the real config in the client-announced workspace root: the workspace config
    // is the nearest one, so it must win. A server resolving config against its process cwd would
    // pick the 8-space decoy.
    let mut context = setup_test(&[
        ("../roughly.toml", "[format]\nindent-width = 8\n"),
        ("roughly.toml", "[format]\nindent-width = 4\n"),
    ])
    .await;

    let file_uri = context.file_uri("R/root.R");
    context
        .open_file(&file_uri, "f <- function(x) {\nx + 1\n}\n")
        .await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let formatted = formatted_text(&mut context, &file_uri).await;
    // Exact-line assertion: a substring check would pass vacuously under the 8-space decoy too
    // ("        x + 1" contains "    x + 1").
    assert!(
        formatted.lines().any(|line| line == "    x + 1"),
        "expected the workspace root's 4-space config to govern, got:\n{formatted}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn ancestor_config_governs_a_workspace_without_its_own() {
    // No config in the workspace root itself: discovery walks up and finds the one in the parent
    // directory, matching the CLI's nearest-ancestor behavior. The ancestor asks for an indent
    // width the built-in default (2) would never produce, so the assertion cannot pass vacuously.
    let mut context = setup_test(&[("../roughly.toml", "[format]\nindent-width = 6\n")]).await;

    let file_uri = context.file_uri("R/ancestor.R");
    context
        .open_file(&file_uri, "f <- function(x) {\nx + 1\n}\n")
        .await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    let formatted = formatted_text(&mut context, &file_uri).await;
    assert!(
        formatted.lines().any(|line| line == "      x + 1"),
        "expected the ancestor config's 6-space indent to govern the workspace, got:\n{formatted}"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn malformed_config_falls_back_to_defaults_and_reports() {
    // Setup completing at all proves a malformed `roughly.toml` no longer aborts server startup.
    let mut context = setup_test(&[("roughly.toml", "[check]\nstrict = \"yes\"\n")]).await;

    let message = context
        .received_message()
        .await
        .expect("expected a config error message after initialization");
    assert_eq!(message.typ, MessageType::ERROR);
    assert!(
        message.message.contains("roughly.toml"),
        "expected the message to name the config file, got: {}",
        message.message
    );
    assert!(
        message.message.contains("line 2"),
        "expected the message to point at the offending line, got: {}",
        message.message
    );
    assert!(
        message.message.contains("expected a boolean"),
        "expected the message to describe the type error, got: {}",
        message.message
    );

    // The server keeps serving on the default configuration.
    let file_uri = context.file_uri("R/alive.R");
    context.open_file(&file_uri, "x <- T\n").await;
    let diagnostics = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("TRUE")),
        "expected the server to keep serving diagnostics, got: {:?}",
        diagnostics.diagnostics
    );

    context.shutdown().await;
}

#[tokio::test]
async fn config_reload_refreshes_push_diagnostics() {
    // Type errors are off by default, so the open document starts clean; enabling `[check] typing`
    // via a config reload must republish the open document's diagnostics without any edit.
    let mut context = setup_test(&[]).await;

    let file_uri = context.file_uri("R/toggle.R");
    context.open_file(&file_uri, "x <- 1L + \"a\"\n").await;

    let initial = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        initial.diagnostics.is_empty(),
        "type errors are off by default, got: {:?}",
        initial.diagnostics
    );

    std::fs::write(
        context.workspace_dir.join("roughly.toml"),
        "[check]\ntyping = true\n",
    )
    .expect("failed to write config");
    context.notify_watched_file_changed("roughly.toml", FileChangeType::CREATED);

    let refreshed = recv_diagnostics(&mut context.diagnostics_receiver, &file_uri, TIMEOUT).await;
    assert!(
        !refreshed.diagnostics.is_empty(),
        "expected the newly enabled type check to surface diagnostics without an edit"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn config_reload_requests_refresh_for_pull_clients() {
    let mut context = setup_test_with_pull_and_refresh(&[]).await;

    let file_uri = context.file_uri("R/pull_toggle.R");
    context.open_file(&file_uri, "x <- 1L\n").await;

    std::fs::write(
        context.workspace_dir.join("roughly.toml"),
        "[check]\ntyping = true\n",
    )
    .expect("failed to write config");
    context.notify_watched_file_changed("roughly.toml", FileChangeType::CREATED);

    assert!(
        context.received_refresh().await,
        "a config reload must ask pull clients to re-pull diagnostics"
    );

    context.shutdown().await;
}

#[tokio::test]
async fn config_reload_failure_keeps_previous_config_and_reports() {
    let mut context = setup_test(&[("roughly.toml", "[format]\nindent-width = 4\n")]).await;

    let file_uri = context.file_uri("R/keep.R");
    context
        .open_file(&file_uri, "f <- function(x) {\nx + 1\n}\n")
        .await;
    drain_diagnostics(&mut context.diagnostics_receiver).await;

    std::fs::write(
        context.workspace_dir.join("roughly.toml"),
        "[format]\nindent-widht = 8\n",
    )
    .expect("failed to write config");
    context.notify_watched_file_changed("roughly.toml", FileChangeType::CHANGED);

    let message = context
        .received_message()
        .await
        .expect("expected a reload error message");
    assert!(
        message.message.contains("unknown field `indent-widht`"),
        "expected the message to name the unknown key, got: {}",
        message.message
    );
    assert!(
        message
            .message
            .contains("keeping the previous configuration"),
        "expected the message to say the old config stays, got: {}",
        message.message
    );

    let formatted = formatted_text(&mut context, &file_uri).await;
    assert!(
        formatted.contains("    x + 1"),
        "expected the previous 4-space config to remain in effect, got:\n{formatted}"
    );

    context.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stub_type_name_jumps_to_its_type_declaration() {
    let mut context = setup_test(&[]).await;

    let stub_uri = context.file_uri("stubs/project.Rtypes");
    context
        .open_file(
            &stub_uri,
            "@type frame\nload_it : fn(x: character) -> frame\n",
        )
        .await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &stub_uri, TIMEOUT).await;

    let response = context
        .server
        .definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: stub_uri.clone(),
                },
                // Inside `frame` in the return position of the declaration line.
                position: Position::new(1, 34),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("definition failed")
        .expect("expected a definition");

    let GotoDefinitionResponse::Scalar(location) = response else {
        panic!("expected a scalar location: {response:?}");
    };
    assert_eq!(location.range.start.line, 0);

    context.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stub_documents_get_semantic_tokens() {
    let mut context = setup_test(&[]).await;

    let stub_uri = context.file_uri("stubs/project.Rtypes");
    context
        .open_file(
            &stub_uri,
            "@type frame\nload_it : fn(x: character) -> frame\n",
        )
        .await;
    let _ = recv_diagnostics(&mut context.diagnostics_receiver, &stub_uri, TIMEOUT).await;

    let tokens = context
        .server
        .semantic_tokens_full(SemanticTokensParams {
            text_document: TextDocumentIdentifier {
                uri: stub_uri.clone(),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .expect("semantic tokens failed");

    let Some(SemanticTokensResult::Tokens(tokens)) = tokens else {
        panic!("expected tokens, got: {tokens:?}");
    };
    assert!(
        tokens.data.len() >= 6,
        "directive + type name + declaration tokens expected: {:?}",
        tokens.data
    );
    // First token is the `@type` directive at line 0, column 0, length 5.
    assert_eq!(tokens.data[0].delta_line, 0);
    assert_eq!(tokens.data[0].delta_start, 0);
    assert_eq!(tokens.data[0].length, 5);

    context.shutdown().await;
}

#[tokio::test]
async fn stub_documents_are_served_with_parse_diagnostics() {
    let mut context = setup_test(&[]).await;

    let stub_uri = context.file_uri("stubs/project.Rtypes");
    context
        .open_file(
            &stub_uri,
            "length : fn(x: Any) -> integer\nbroken line without colon\n",
        )
        .await;

    let published = recv_diagnostics(&mut context.diagnostics_receiver, &stub_uri, TIMEOUT).await;
    assert_eq!(
        published.diagnostics.len(),
        1,
        "one malformed line, one diagnostic: {:?}",
        published.diagnostics
    );
    assert_eq!(published.diagnostics[0].range.start.line, 1);

    // Fixing the line republishes empty diagnostics.
    context.change_file(
        &stub_uri,
        1,
        Range::new(Position::new(1, 0), Position::new(1, 25)),
        "nchar : fn(x: character) -> integer",
    );
    let after_fix = recv_diagnostics(&mut context.diagnostics_receiver, &stub_uri, TIMEOUT).await;
    assert!(
        after_fix.diagnostics.is_empty(),
        "fixed stub publishes no diagnostics, got: {:?}",
        after_fix.diagnostics
    );

    // A declaration that parses but names an unresolvable type fails to harvest; the loader would
    // drop it silently, so the served diagnostics must call it out.
    context.change_file(
        &stub_uri,
        2,
        Range::new(Position::new(1, 0), Position::new(1, 35)),
        "bad : Frobnicate",
    );
    let after_harvest_failure =
        recv_diagnostics(&mut context.diagnostics_receiver, &stub_uri, TIMEOUT).await;
    assert_eq!(
        after_harvest_failure.diagnostics.len(),
        1,
        "one unharvestable declaration, one diagnostic: {:?}",
        after_harvest_failure.diagnostics
    );
    assert!(
        after_harvest_failure.diagnostics[0]
            .message
            .contains("does not load"),
        "message should say the declaration does not load: {:?}",
        after_harvest_failure.diagnostics[0].message
    );

    context.shutdown().await;
}

#[tokio::test]
async fn stub_documents_answer_pull_diagnostics() {
    // A pull client gets stub-buffer reports through `textDocument/diagnostic` (no push), with the
    // same unchanged/full result-id protocol as R documents.
    let mut context = setup_test_with_pull_diagnostics(&[]).await;

    let stub_uri = context.file_uri("stubs/project.Rtypes");
    context
        .open_file(
            &stub_uri,
            "length : fn(x: Any) -> integer\nbroken line without colon\n",
        )
        .await;

    let (messages, result_id) =
        full_report_messages(context.document_diagnostic(&stub_uri, None).await);
    assert_eq!(
        messages.len(),
        1,
        "one malformed line, one diagnostic: {messages:?}"
    );
    let result_id = result_id.expect("a non-empty stub report carries a result id");

    let repeat = context
        .document_diagnostic(&stub_uri, Some(result_id.clone()))
        .await;
    assert!(
        matches!(
            repeat,
            DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_))
        ),
        "expected an unchanged report on repeat pull, got: {repeat:?}"
    );

    // Fixing the line moves the next pull to a clean full report.
    context.change_file(
        &stub_uri,
        1,
        Range::new(Position::new(1, 0), Position::new(1, 25)),
        "nchar : fn(x: character) -> integer",
    );
    let (after_fix, _) = full_report_messages(context.document_diagnostic(&stub_uri, None).await);
    assert!(
        after_fix.is_empty(),
        "fixed stub pulls an empty report, got: {after_fix:?}"
    );

    // A stub path that was never opened answers an empty full report like any untracked pull.
    let never_opened = context.file_uri("stubs/never_opened.Rtypes");
    let (unopened, _) =
        full_report_messages(context.document_diagnostic(&never_opened, None).await);
    assert!(
        unopened.is_empty(),
        "expected an empty report for an unopened stub, got: {unopened:?}"
    );

    context.shutdown().await;
}
