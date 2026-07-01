use {
    crate::{
        cli,
        config::{Config, ExperimentalFeatures},
        diagnostics, format,
        index::{self, IndexError, Item},
        lsp_types::{
            CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionList,
            CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticOptions,
            DiagnosticServerCapabilities, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
            DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
            DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentDiagnosticParams,
            DocumentDiagnosticReport, DocumentDiagnosticReportResult, DocumentFormattingParams,
            DocumentRangeFormattingParams, DocumentSymbol, DocumentSymbolParams,
            DocumentSymbolResponse, FileChangeType, FileSystemWatcher,
            FullDocumentDiagnosticReport, GlobPattern, Hover, HoverContents, HoverParams,
            HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
            InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, Location, MarkupContent,
            MarkupKind, MessageType, OneOf, ParameterInformation, ParameterLabel, Position,
            PublishDiagnosticsParams, Range, ReferenceParams, Registration, RegistrationParams,
            RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
            RelativePattern, RenameParams, SaveOptions, SemanticToken, SemanticTokenType,
            SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
            SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
            ServerCapabilities, ServerInfo, ShowMessageParams, SignatureHelp, SignatureHelpOptions,
            SignatureHelpParams, SignatureInformation, TextDocumentSyncCapability,
            TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
            UnchangedDocumentDiagnosticReport, Url, WorkspaceEdit, WorkspaceSymbolParams,
            WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
        },
        position::{self, PositionEncoding},
        symbols,
    },
    analysis::{
        self, Document, DocumentChange, TextPosition, TextRange, ide, naming::DocumentKind,
    },
    async_lsp::{
        ClientSocket, ErrorCode, LanguageClient, LanguageServer, ResponseError,
        client_monitor::ClientProcessMonitorLayer,
        lsp_types::{DidChangeConfigurationParams, GotoDefinitionParams, GotoDefinitionResponse},
        panic::CatchUnwindLayer,
        router::Router,
        server::LifecycleLayer,
        tracing::TracingLayer,
    },
    engine::{
        Cancelled, Engine, Shared,
        ide_view::{EngineIde, PathTable},
        queries::{
            Config as EngineConfig, FileDiagnostics, FileId, Key, ParsedDocument, RoughlyQueries,
        },
    },
    futures::future::BoxFuture,
    std::{
        collections::{HashMap, HashSet, hash_map::DefaultHasher},
        hash::{Hash, Hasher},
        ops::ControlFlow,
        panic::{AssertUnwindSafe, catch_unwind},
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Instant,
    },
    tokio::sync::oneshot,
    tower::ServiceBuilder,
    tree_sitter::Point,
};

const CONFIG_FILE_NAME: &str = "roughly.toml";

// #[tokio::main] # TODO: understand if this makes a difference???
#[tokio::main(flavor = "current_thread")]
pub async fn run(experimental_features: ExperimentalFeatures) {
    install_panic_hook();
    let runtime = tokio::runtime::Handle::current();

    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        let config = Config::from_path(Path::new(CONFIG_FILE_NAME)).unwrap_or_else(|error| {
            cli::error(&error.to_string());
            panic!("failed to load config: {error}");
        });

        // The `!Send` engine lives on a dedicated worker thread, built INSIDE the closure (it cannot be
        // moved across threads). The frontend forwards every LSP op to it over the channel + shares the
        // cancellation token.
        let (sender, receiver) = mpsc::channel::<Job>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_client = client.clone();
        let worker_cancel = cancel.clone();
        let worker_runtime = runtime.clone();
        std::thread::Builder::new()
            .name("roughly-engine".to_owned())
            .spawn(move || {
                EngineWorker::new(
                    worker_client,
                    config,
                    experimental_features,
                    worker_cancel,
                    worker_runtime,
                )
                .run(receiver);
            })
            .expect("engine worker thread should spawn");

        // No `ConcurrencyLayer`: the serial worker IS the concurrency bound, and that layer's poll_ready
        // backpressure deadlocks pending request futures against MainLoop's inner dispatch loop (it awaits
        // the gate without polling the in-flight task set). Dropping it loses only `$/cancelRequest`
        // early-abort, which the edit-driven cancellation token supersedes.
        ServiceBuilder::new()
            .layer(TracingLayer::default())
            .layer(LifecycleLayer::default())
            .layer(CatchUnwindLayer::default())
            .layer(ClientProcessMonitorLayer::new(client.clone()))
            .service(Router::from_language_server(ServerState { sender, cancel }))
    });

    // Prefer truly asynchronous piped stdin/stdout without blocking tasks.
    #[cfg(unix)]
    let (stdin, stdout) = (
        async_lsp::stdio::PipeStdin::lock_tokio().unwrap(),
        async_lsp::stdio::PipeStdout::lock_tokio().unwrap(),
    );
    // Fallback to spawn blocking read/write otherwise.
    #[cfg(not(unix))]
    let (stdin, stdout) = (
        tokio_util::compat::TokioAsyncReadCompatExt::compat(tokio::io::stdin()),
        tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(tokio::io::stdout()),
    );

    tracing::info!("starting server using async-lsp ...");
    server.run_buffered(stdin, stdout).await.unwrap();
}

// The analysis backend, running on its own dedicated thread (the engine is `!Send`/`!Sync`). The
// async-lsp frontend (`ServerState`) forwards every LSP op here as a `Job` over an mpsc channel; this
// is the single owner of the engine + open-document buffers + path tables, so all engine access is
// single-threaded. See the "off-thread + cancellation" design in the decisions record.
struct EngineWorker {
    client: ClientSocket,
    // Cancellation token shared with the frontend: the frontend stores `true` before every input-
    // mutating notification; the worker stores `false` at the start of each read and installs it around
    // the read's engine fetches, so a newer edit abandons an in-flight read (latest-edit-wins).
    cancel: Arc<AtomicBool>,
    // The tokio runtime handle, to spawn the rare async client-requests (capability registration,
    // workspace diagnostic refresh) — a `std::thread` worker cannot `.await` directly.
    runtime: tokio::runtime::Handle,
    config: Config,
    experimental_features: ExperimentalFeatures,
    workspace_root: PathBuf,
    open_documents: HashSet<PathBuf>,
    // The memoized query engine: the single analysis backend. Document text is fed in as `SourceText`
    // inputs from the open-document buffers (`documents`) and from disk for closed package files.
    // `paths`/`file_ids` are the host-owned path↔FileId bijection the engine keys on (the engine itself
    // never sees paths).
    engine: Engine<RoughlyQueries>,
    paths: PathTable,
    file_ids: HashMap<PathBuf, FileId>,
    next_file_id: FileId,
    // The open-document edit buffers: the server owns the LSP document text for open files,
    // reusing `Document::{parse, edit}` for incremental change application. `did_change` resolves each
    // change's range against this evolving buffer. Closed package documents are not buffered here — they
    // exist only as engine `SourceText` inputs read from disk.
    documents: HashMap<PathBuf, Document>,
    parser: tree_sitter::Parser,
    position_encoding: PositionEncoding,
    // When the client advertises pull-diagnostics support it owns the request cadence, so the
    // server stops pushing `publish_diagnostics` and answers `textDocument/diagnostic` instead.
    client_supports_pull_diagnostics: bool,
    // When a pull client also supports `workspace/diagnostic/refresh`, a package-visible save that
    // moves diagnostics in OTHER documents asks the client to re-pull. Push is suppressed for pull
    // clients, so without this their non-visible dependents would go stale after such a save.
    client_supports_diagnostic_refresh: bool,
}

// One LSP operation handed to the worker. A `Read` (a query handler) resets the cancellation token at
// the start so a newer edit can abandon it; a `Write` (a lifecycle/edit notification, or the mutating
// `initialize`) runs to completion. Both carry a boxed closure that runs the migrated handler on the
// worker's `&mut EngineWorker` and (for requests) sends the reply back over a oneshot.
enum Job {
    Read(Box<dyn FnOnce(&mut EngineWorker) + Send>),
    Write(Box<dyn FnOnce(&mut EngineWorker) + Send>),
}

impl EngineWorker {
    // Built INSIDE the worker thread closure (the engine is `!Send`, so it cannot be constructed on the
    // runtime thread and moved). The closure captures only `Send` inputs (client, cancel, runtime, config).
    fn new(
        client: ClientSocket,
        config: Config,
        experimental_features: ExperimentalFeatures,
        cancel: Arc<AtomicBool>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let workspace_root = std::env::current_dir().unwrap();

        // A project may ship its own `.Rtypes` stubs under `<root>/stubs/` to override or extend the
        // shipped standard-library corpus. They are read once here and folded into the engine's set-once
        // stub library; they are never re-read on an edit.
        let project_stub_sources = analysis::stdlib::discover_project_stub_sources(&workspace_root);

        Self {
            client,
            cancel,
            runtime,
            config,
            experimental_features,
            workspace_root: workspace_root.clone(),
            open_documents: HashSet::new(),
            engine: Engine::new(RoughlyQueries::with_project_stubs(project_stub_sources)),
            paths: PathTable::new(workspace_root.clone()),
            file_ids: HashMap::new(),
            next_file_id: 0,
            documents: HashMap::new(),
            parser: analysis::tree::new_parser().expect("server parser should initialize"),
            position_encoding: PositionEncoding::Utf16,
            client_supports_pull_diagnostics: false,
            client_supports_diagnostic_refresh: false,
        }
    }

    fn workspace_r_path(&self) -> PathBuf {
        self.workspace_root.join("R")
    }

    fn document(&self, path: &Path) -> Option<&Document> {
        self.documents.get(path)
    }

    // The engine's parsed document (rope + tree) for a path, or None if the path is not a tracked file.
    // Used to encode OUTGOING ranges — which may target a closed cross-file document the engine still holds,
    // where the open-buffer set is insufficient — against the correct document's rope.
    fn parsed_for(&self, path: &Path) -> Option<Shared<ParsedDocument>> {
        let file = self.file_ids.get(path).copied()?;
        Some(self.engine.fetch::<ParsedDocument>(Key::Parse(file)))
    }

    // Run an engine read under the shared cancellation token (the worker resets it to `false` before each
    // `Job::Read`). If a newer edit flips it mid-read, the engine abandons the in-flight computation and
    // this returns the type's empty default, so the query answers empty rather than spending a full pass
    // on a result the next edit already superseded — latest-edit-wins. Only used by read handlers; edit
    // handlers run their (different-file-publishing) diagnostics to completion via the plain `fetch` path.
    fn cancellable<R: Default>(&self, body: impl FnOnce() -> R) -> R {
        self.engine
            .with_cancellation(self.cancel.clone(), body)
            .unwrap_or_default()
    }

    fn opened_document(&self, path: &Path) -> Option<&Document> {
        self.documents.get(path)
    }

    fn to_internal_position(&self, path: &Path, position: Position) -> Option<TextPosition> {
        let rope = self.document(path)?.rope();
        Some(position::lsp_position_to_internal(
            rope,
            self.position_encoding,
            position,
        ))
    }

    fn to_internal_range(&self, path: &Path, range: Range) -> Option<TextRange> {
        let rope = self.document(path)?.rope();
        Some(position::lsp_range_to_internal(
            rope,
            self.position_encoding,
            range,
        ))
    }

    // The target of a definition, reference, or rename edit may live in a different document than
    // the request, so the outgoing range is encoded against that document's rope.
    fn to_lsp_range_in(&self, path: &Path, range: TextRange) -> Range {
        match self.parsed_for(path) {
            Some(parsed) => {
                position::internal_range_to_lsp(parsed.0.rope(), self.position_encoding, range)
            }
            None => Range::new(
                Position::new(
                    range.start.line_index as u32,
                    range.start.character_index as u32,
                ),
                Position::new(
                    range.end.line_index as u32,
                    range.end.character_index as u32,
                ),
            ),
        }
    }

    fn to_lsp_position_in(&self, path: &Path, position: TextPosition) -> Position {
        match self.parsed_for(path) {
            Some(parsed) => position::internal_position_to_lsp(
                parsed.0.rope(),
                self.position_encoding,
                position,
            ),
            None => Position::new(position.line_index as u32, position.character_index as u32),
        }
    }

    // Diagnostics are served by the engine: map the path to its engine `FileId` and render the engine's
    // `FileDiagnostics`, gating the typing/strict/unused classes by config exactly as production's
    // `document_diagnostics` does. Type errors stay raw and are rendered here against the engine's interner +
    // fallback range (single source for the error set).
    fn convert_document_diagnostics(&self, path: &Path) -> Vec<crate::lsp_types::Diagnostic> {
        let file = self
            .file_ids
            .get(path)
            .copied()
            .expect("diagnostics document present in engine file ids");

        let file_diagnostics = self.engine.fetch::<FileDiagnostics>(Key::Diagnostics(file));
        let fallback = *self.engine.fetch::<tree_sitter::Range>(Key::FallbackRange);
        let config = self.engine_config();

        let mut rendered = Vec::new();
        // Local naming, package naming, lowering (syntax), and lint are emitted unconditionally, exactly as
        // production's `document_diagnostics` does (they are not config-gated).
        rendered.extend(file_diagnostics.naming.iter().cloned());
        rendered.extend(file_diagnostics.package_naming.iter().cloned());
        rendered.extend(file_diagnostics.lowering.iter().cloned());
        rendered.extend(file_diagnostics.lint.iter().cloned());
        if config.unused {
            rendered.extend(file_diagnostics.unused.iter().cloned());
        }
        if config.typing {
            self.engine.group().with_interner(|interner| {
                for error in &file_diagnostics.type_errors {
                    rendered.push(analysis::Diagnostic::from_inference_error(
                        error, fallback, interner,
                    ));
                }
            });
        }
        if config.strict {
            rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
        }

        let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
        diagnostics::convert_diagnostics(rendered, parsed.0.rope(), self.position_encoding)
    }

    fn convert_location(&self, location: analysis::ide::Location) -> Location {
        Location {
            uri: Url::from_file_path(&location.path).expect("location path should convert to URI"),
            range: self.to_lsp_range_in(&location.path, location.range),
        }
    }

    // Workspace symbols index every package document's tree (the `R/` files). The host already knows which
    // `FileId`s are package documents (path under `R/`); each tree comes from the engine's `Parse` query.
    fn package_items_map(&self) -> HashMap<PathBuf, Vec<Item>> {
        let r_path = self.workspace_r_path();
        let package = self
            .file_ids
            .iter()
            .filter(|(path, _)| path.starts_with(&r_path))
            .map(|(path, file)| (path.clone(), *file))
            .collect::<Vec<_>>();
        package
            .into_iter()
            .map(|(path, file)| {
                let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
                let items =
                    index::index(parsed.0.tree().root_node(), parsed.0.rope(), false, false);
                (path, items)
            })
            .collect()
    }

    fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            typing: self.config.check.typing,
            strict: self.config.check.strict,
            unused: self.config.check.unused,
            lint: self.config.lint,
        }
    }

    fn file_id_for(&mut self, path: &Path) -> FileId {
        if let Some(file) = self.file_ids.get(path) {
            return *file;
        }
        let file = self.next_file_id;
        self.next_file_id += 1;
        self.file_ids.insert(path.to_path_buf(), file);
        file
    }

    fn is_package_path(&self, path: &Path) -> bool {
        path.starts_with(self.workspace_r_path())
    }

    // Set (or update) a file's engine source inputs. Does not touch `ProjectFiles`; the caller invokes
    // `rebuild_project_files` when the file SET changes (add/remove), not on a text-only edit.
    fn set_source_input(&mut self, path: &Path, text: String, is_package: bool) {
        let file = self.file_id_for(path);
        self.engine.set_input(Key::SourceText(file), text);
        self.engine.set_input(
            Key::DocumentKind(file),
            if is_package {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            },
        );
        self.paths.insert(file, path.to_path_buf());
    }

    // Drop a file from the engine (deletion or a closed, no-longer-tracked document): tombstone its source
    // input so dependents revalidate against the smaller set, and remove it from the host tables.
    fn retract_source_input(&mut self, path: &Path) {
        if let Some(file) = self.file_ids.remove(path) {
            self.engine.remove_input(&Key::SourceText(file));
            self.engine.remove_input(&Key::DocumentKind(file));
            self.paths.remove(file);
        }
    }

    // Recompute the `ProjectFiles` input. Package documents come first, ascending by package-relative path
    // key, so the engine's last-writer-wins symbol index selects the same winner production's
    // `max_by_key(package_path_key)` does; scripts follow (they export nothing, so their order is irrelevant).
    fn rebuild_project_files(&mut self) {
        let r_path = self.workspace_r_path();
        let workspace_root = self.workspace_root.clone();
        let mut entries = self
            .file_ids
            .iter()
            .map(|(path, file)| {
                let is_package = path.starts_with(&r_path);
                let key = path
                    .strip_prefix(&workspace_root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/");
                (is_package, key, *file)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        let project = entries
            .into_iter()
            .map(|(_, _, file)| file)
            .collect::<Vec<_>>();
        self.engine.set_input(Key::ProjectFiles, project);
    }

    fn report_error(&self, message: String) {
        if let Err(error) = self.client.clone().show_message(ShowMessageParams {
            typ: MessageType::ERROR,
            message,
        }) {
            tracing::error!(?error, "failed to send error message to client");
        }
    }
}

impl EngineWorker {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, ResponseError> {
        tracing::info!(?self.experimental_features, "initialize");

        let client_encodings = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_deref());
        self.position_encoding = PositionEncoding::negotiate(client_encodings);
        tracing::info!(?self.position_encoding, "negotiated position encoding");

        self.client_supports_pull_diagnostics = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.diagnostic.as_ref())
            .is_some();
        self.client_supports_diagnostic_refresh = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.diagnostic.as_ref())
            .and_then(|diagnostic| diagnostic.refresh_support)
            .unwrap_or(false);
        tracing::info!(
            pull_diagnostics = self.client_supports_pull_diagnostics,
            diagnostic_refresh = self.client_supports_diagnostic_refresh,
            "negotiated diagnostics delivery"
        );

        let workspace_r_path = self.workspace_r_path();

        if workspace_r_path.is_dir() {
            match index::source_file_paths(&workspace_r_path) {
                Ok(paths) => {
                    for path in paths {
                        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                            panic!("failed to read package source {}: {error}", path.display())
                        });
                        self.set_source_input(&path, text, true);
                    }
                }
                Err(IndexError) => {
                    panic!(
                        "failed to list package source files in {}",
                        workspace_r_path.display()
                    );
                }
            }
        }

        self.rebuild_project_files();
        self.engine.set_input(Key::Config, self.engine_config());

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(self.position_encoding.kind()),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["$".into(), "@".into(), ":".into()]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("roughly".into()),
                        inter_file_dependencies: true,
                        workspace_diagnostics: false,
                        work_done_progress_options: Default::default(),
                    },
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(
                    self.experimental_features.range_formatting,
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: semantic_token_legend(),
                                token_modifiers: Vec::new(),
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            work_done_progress_options: Default::default(),
                        },
                    ),
                ),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    fn initialized(&mut self, _: InitializedParams) {
        let workspace_r_path = self.workspace_r_path();

        let params = RegistrationParams {
            registrations: vec![Registration {
                id: DidChangeWatchedFiles::METHOD.into(),
                method: DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(
                                        Url::from_file_path(&workspace_r_path).unwrap(),
                                    ),
                                    pattern: "*.[rR]".into(),
                                }),
                                kind: None,
                            },
                            FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(
                                        Url::from_file_path(&self.workspace_root).unwrap(),
                                    ),
                                    pattern: CONFIG_FILE_NAME.into(),
                                }),
                                kind: None,
                            },
                        ],
                    })
                    .unwrap(),
                ),
            }],
        };

        let mut client = self.client.clone();
        self.runtime.spawn(async move {
            if let Err(err) = client.register_capability(params).await {
                if let Err(error) = client.show_message(ShowMessageParams {
                    typ: MessageType::ERROR,
                    message: format!("failed to watch R files: {err:#}"),
                }) {
                    tracing::error!(?error, "failed to notify client of file-watch failure");
                }
                return;
            }
            tracing::info!("registered file watching for R files");
        });
    }

    //
    // TEXT SYNC
    //

    fn did_open(&mut self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let text = &params.text_document.text;

        tracing::debug!(?path, "did open");

        let document = Document::parse(&mut self.parser, text)
            .unwrap_or_else(|_| panic!("failed to parse open document buffer {}", path.display()));
        self.documents.insert(path.clone(), document);
        self.open_documents.insert(path.clone());
        let is_package = self.is_package_path(&path);
        self.set_source_input(&path, text.clone(), is_package);
        self.rebuild_project_files();

        if !self.client_supports_pull_diagnostics {
            let diagnostics = self.convert_document_diagnostics(&path);
            if let Err(error) = self
                .client
                .publish_diagnostics(PublishDiagnosticsParams::new(
                    uri,
                    diagnostics,
                    Some(params.text_document.version),
                ))
            {
                tracing::error!(?error, "failed to publish diagnostics");
            }
        }
    }

    fn did_close(&mut self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "did close");

        self.open_documents.remove(&path);
        self.documents.remove(&path);
        if self.is_package_path(&path) && path.exists() {
            // A closed package file still on disk reverts to its on-disk text (discarding unsaved buffer
            // edits); the file set is unchanged, so no `rebuild_project_files`.
            let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "failed to reload package source on close {}: {error}",
                    path.display()
                )
            });
            self.set_source_input(&path, text, true);
        } else {
            // A deleted package file, or a closed script: it is no longer tracked.
            self.retract_source_input(&path);
            self.rebuild_project_files();
        }
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();
        let content_changes = params.content_changes;

        tracing::debug!(?path, "did change");

        let start = Instant::now();

        if !self.open_documents.contains(&path) {
            self.report_error(format!(
                "received did_change for non-open document {}",
                path.display()
            ));
            return;
        }

        self.document(&path)
            .unwrap_or_else(|| panic!("open document not found for {}", path.display()));

        // Each incremental change's range refers to the document state after the previous changes
        // in the batch are applied, so positions are converted and applied against the live rope
        // one change at a time rather than all up front.
        for change in content_changes {
            let range = change.range.unwrap_or_else(|| {
                panic!(
                    "incremental did_change for {} must include a range",
                    path.display()
                )
            });
            let internal_range = self.to_internal_range(&path, range).unwrap_or_else(|| {
                panic!(
                    "analysis document not found while converting did_change range for {}",
                    path.display()
                )
            });
            let document_change = DocumentChange {
                range: internal_range,
                text: change.text,
            };
            // Each change's range is resolved against the buffer AFTER the previous changes (via
            // `to_internal_range` -> `document()` -> `documents`), so apply it before the next iteration.
            self.documents
                .get_mut(&path)
                .expect("open document buffer present for did_change")
                .edit(&mut self.parser, std::slice::from_ref(&document_change))
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to edit open document buffer {} incrementally: {error:?}",
                        path.display()
                    )
                });
        }
        // A text-only edit leaves the file SET unchanged, so only the file's `SourceText` is re-set (no
        // `rebuild_project_files`); the buffer is the source of truth for the new text.
        let text = self
            .documents
            .get(&path)
            .expect("open document buffer present after did_change")
            .rope()
            .to_string();
        let is_package = self.is_package_path(&path);
        self.set_source_input(&path, text, is_package);

        if !self.client_supports_pull_diagnostics {
            let diagnostics = self.convert_document_diagnostics(&path);
            if let Err(error) = self
                .client
                .publish_diagnostics(PublishDiagnosticsParams::new(
                    uri.clone(),
                    diagnostics,
                    Some(params.text_document.version),
                ))
            {
                tracing::error!(?error, "failed to publish diagnostics");
            }
        }

        tracing::debug!(elapsed = start.elapsed().as_millis());
    }

    fn did_save(&mut self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let path = uri.to_file_path().unwrap();

        tracing::debug!(?path, "did save");

        if !self.open_documents.contains(&path) {
            self.report_error(format!(
                "received did_save for non-open document {}",
                path.display()
            ));
            return;
        }

        // Coherence: a saved document is open, so it must be a tracked engine file.
        assert!(
            self.file_ids.contains_key(&path),
            "saved document not tracked {}",
            path.display()
        );

        // Pull clients re-request the saved (visible) document on their own cadence, so the server
        // does not push. But push is suppressed for them, so a save that moved diagnostics in OTHER
        // (non-visible) documents would leave those dependents stale. Precisely detecting which
        // dependents moved is not reliably available here — `run_full` reports an affected set only
        // when type checking is enabled, yet cross-file naming diagnostics (e.g. "could not resolve")
        // move regardless — so conservatively ask the client to re-pull on every save. Saves are
        // infrequent and the client only re-pulls the documents it has open.
        if self.client_supports_pull_diagnostics {
            if self.client_supports_diagnostic_refresh {
                let mut client = self.client.clone();
                self.runtime.spawn(async move {
                    if let Err(error) = client.workspace_diagnostic_refresh(()).await {
                        tracing::error!(?error, "failed to request workspace diagnostic refresh");
                    }
                });
            }
            return;
        }

        // A package-visible save can move diagnostics in dependent files, and live edits push only the
        // edited document, so on save every OPEN document is republished from the engine. The engine
        // already reflects the synced text (the edit inputs were set as the changes arrived), so each open
        // document's current diagnostics are correct — this is a superset of the old typecheck-affected set
        // and also catches naming-only dependents the affected set missed.
        for open_path in self.open_documents.clone() {
            if !self.file_ids.contains_key(&open_path) {
                continue;
            }
            let open_uri = if open_path == path {
                uri.clone()
            } else {
                match Url::from_file_path(&open_path) {
                    Ok(open_uri) => open_uri,
                    Err(()) => continue,
                }
            };
            let diagnostics = self.convert_document_diagnostics(&open_path);
            if let Err(error) = self
                .client
                .publish_diagnostics(PublishDiagnosticsParams::new(open_uri, diagnostics, None))
            {
                tracing::error!(?error, "failed to publish diagnostics");
            }
        }
    }

    fn did_change_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        let config_path = self.workspace_root.join(CONFIG_FILE_NAME);
        let workspace_r_path = self.workspace_r_path();

        for change in params.changes {
            let uri = change.uri;
            let typ = change.typ;
            let path = uri.to_file_path().unwrap();

            tracing::info!(?path, ?typ, "watched file changed");

            if path == config_path {
                match Config::from_path(&config_path) {
                    Ok(config) => {
                        self.config = config;
                    }
                    Err(error) => {
                        self.report_error(format!("failed to reload config: {error}"));
                    }
                }
                continue;
            }

            // An open document's text comes from the editor (its buffer), not the watcher, so on-disk
            // changes to open files are ignored here.
            if path.starts_with(&workspace_r_path) && !self.open_documents.contains(&path) {
                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        // A watcher event can race the filesystem (the file is renamed or removed
                        // between the notification and this read). That is recoverable — the engine
                        // keeps its previous text and a later watcher event re-syncs — so skip the
                        // file rather than crashing the server. This is unlike a `did_change` buffer
                        // edit, whose failure would leave analysis state incoherent.
                        match std::fs::read_to_string(&path) {
                            Ok(text) => self.set_source_input(&path, text, true),
                            Err(error) => {
                                tracing::warn!(
                                    ?path,
                                    ?error,
                                    "failed to read watched package source; skipping"
                                );
                                continue;
                            }
                        }
                    }
                    FileChangeType::DELETED => {
                        self.retract_source_input(&path);
                    }
                    other => {
                        tracing::debug!(?other, "ignoring unhandled watched-file change type");
                    }
                }
            }
        }
        // The file set and/or config may have changed across the batch.
        self.rebuild_project_files();
        self.engine.set_input(Key::Config, self.engine_config());
    }

    fn did_change_configuration(&mut self, _params: DidChangeConfigurationParams) {
        // Stub implementation to satisfy Zed's requirements; does not apply any configuration changes.
    }

    //
    // PULL DIAGNOSTICS
    //

    fn document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult, ResponseError> {
        let uri = params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(empty_full_diagnostic_report());
        };

        tracing::debug!(?path, "document diagnostic");

        // Unlike the sync notifications, a pull can legitimately target a document the server does
        // not track; answer with an empty full report rather than panicking.
        if !self.file_ids.contains_key(&path) {
            tracing::debug!(?path, "pull diagnostic for untracked document");
            return Ok(empty_full_diagnostic_report());
        }

        // The engine computes the report on demand (typecheck included via the `Diagnostics` fetch in
        // `convert_document_diagnostics`), so it already equals the push path and reflects package-visible
        // edits in dependent files — no separate full pass is needed.
        let items = self.cancellable(|| self.convert_document_diagnostics(&path));
        let result_id = diagnostics_result_id(&items);

        // The result id is a content hash of the report, so an unchanged answer is correct even
        // under inter-file dependencies where the document's own edit version did not move.
        if params.previous_result_id.as_deref() == Some(result_id.as_str()) {
            return Ok(DocumentDiagnosticReportResult::Report(
                DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                }),
            ));
        }

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items,
                },
            }),
        ))
    }

    //
    // COMPLETION
    //

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, ResponseError> {
        let Ok(path) = params
            .text_document_position
            .text_document
            .uri
            .to_file_path()
        else {
            return Ok(None);
        };
        let position = params.text_document_position.position;

        tracing::debug!(?path, "completion");

        if self.opened_document(&path).is_none() {
            tracing::error!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for completion");
        let completions = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).completion(&path, internal_position)
            })
            .map(|result| {
                CompletionResponse::List(CompletionList {
                    is_incomplete: result.is_incomplete,
                    items: result
                        .items
                        .into_iter()
                        .map(|item| CompletionItem {
                            label: item.label,
                            label_details: Some(CompletionItemLabelDetails {
                                detail: None,
                                description: Some(match item.source {
                                    analysis::CompletionItemSource::Keyword => "Keyword".into(),
                                    analysis::CompletionItemSource::Local => "Local".into(),
                                    analysis::CompletionItemSource::Global => "Global".into(),
                                }),
                            }),
                            kind: Some(match item.kind {
                                analysis::CompletionItemKind::Keyword => {
                                    CompletionItemKind::KEYWORD
                                }
                                analysis::CompletionItemKind::Variable => {
                                    CompletionItemKind::VARIABLE
                                }
                                analysis::CompletionItemKind::Function => {
                                    CompletionItemKind::FUNCTION
                                }
                            }),
                            ..Default::default()
                        })
                        .collect(),
                })
            });

        Ok(completions)
    }

    //
    // DEFINITION
    //

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, ResponseError> {
        let uri = params.text_document_position_params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "goto definition");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for definition");
        let response = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).definition(&path, internal_position)
            })
            .map(|locations| {
                let mut locations = locations
                    .into_iter()
                    .map(|location| self.convert_location(location))
                    .collect::<Vec<_>>();
                match locations.len() {
                    1 => GotoDefinitionResponse::Scalar(
                        locations.pop().expect("single definition location"),
                    ),
                    _ => GotoDefinitionResponse::Array(locations),
                }
            });

        Ok(response)
    }

    //
    // HOVER
    //

    fn hover(&mut self, params: HoverParams) -> Result<Option<Hover>, ResponseError> {
        let uri = params.text_document_position_params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "hover");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for hover");
        let Some(hover_info) = self.cancellable(|| {
            EngineIde::new(&self.engine, &self.paths).hover(&path, internal_position)
        }) else {
            tracing::debug!(?position, "hover target not found");
            return Ok(None);
        };

        let value = ide::render_hover_markdown(&hover_info, self.experimental_features.debug);

        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(self.to_lsp_range_in(&path, hover_info.range)),
        };

        Ok(Some(hover))
    }

    //
    // INLAY HINTS
    //

    fn inlay_hint(
        &mut self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>, ResponseError> {
        let uri = params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };

        tracing::debug!(?path, "inlay hints");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let viewport = self
            .to_internal_range(&path, params.range)
            .expect("opened document rope available for inlay hints");
        let raw_hints = self.cancellable(|| {
            EngineIde::new(&self.engine, &self.paths).inlay_hints(&path, Some(viewport))
        });
        let hints = raw_hints
            .into_iter()
            .map(|hint| InlayHint {
                position: self.to_lsp_position_in(&path, hint.position),
                label: InlayHintLabel::String(hint.label),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: Some(false),
                padding_right: Some(false),
                data: None,
            })
            .collect();

        Ok(Some(hints))
    }

    //
    // SEMANTIC TOKENS
    //

    // Highlights the type notation inside `#:` annotation comments in an `.R` document. The annotation
    // parser discards per-token spans into interned symbols, so the type text is classified directly by
    // `analysis::type_semantic_tokens`; this is a highlighter, not a re-parse. Scope is `#:` annotations
    // in `.R` files only — `.Rtypes` stub files are not served as documents yet, so they are not covered.
    fn semantic_tokens_full(
        &mut self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>, ResponseError> {
        let uri = params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };

        tracing::debug!(?path, "semantic tokens");

        let Some(parsed) = self.parsed_for(&path) else {
            return Ok(None);
        };
        let rope = parsed.0.rope();
        let mut comment_nodes = Vec::new();
        collect_comment_nodes(parsed.0.tree().root_node(), &mut comment_nodes);

        // Absolute (line, utf-position) of the previous emitted token, for the delta encoding the LSP
        // protocol requires.
        let mut previous_line = 0u32;
        let mut previous_start = 0u32;
        let mut data = Vec::new();

        for node in comment_nodes {
            let comment_text = rope
                .byte_slice(node.start_byte()..node.end_byte())
                .to_string();
            let trimmed = comment_text.trim_start();
            if !trimmed.starts_with("#:") {
                continue;
            }
            // The classifier runs on the annotation body after `#:`; offsets it returns are relative to
            // that body, so shift them by the body's byte offset within the comment. A comment is a
            // single line, so the row is fixed and the column is a byte offset on that line.
            let leading_whitespace = comment_text.len() - trimmed.len();
            let Some(body) = trimmed.strip_prefix("#:") else {
                continue;
            };
            let body_offset_in_comment = leading_whitespace + "#:".len();
            let comment_start_row = node.start_position().row;
            let comment_start_column = node.start_position().column;

            for token in analysis::type_semantic_tokens(body) {
                let comment_relative_start = body_offset_in_comment + token.start;
                let length_bytes = token.end - token.start;
                let line_byte_column = comment_start_column + comment_relative_start;
                let start_position = self.to_lsp_position_in(
                    &path,
                    TextPosition {
                        line_index: comment_start_row,
                        character_index: line_byte_column,
                    },
                );
                let end_position = self.to_lsp_position_in(
                    &path,
                    TextPosition {
                        line_index: comment_start_row,
                        character_index: line_byte_column + length_bytes,
                    },
                );
                let line = start_position.line;
                let start_character = start_position.character;
                let length = end_position.character.saturating_sub(start_character);
                if length == 0 {
                    continue;
                }

                let delta_line = line - previous_line;
                let delta_start = if delta_line == 0 {
                    start_character - previous_start
                } else {
                    start_character
                };
                data.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length,
                    token_type: semantic_token_index(token.role),
                    token_modifiers_bitset: 0,
                });
                previous_line = line;
                previous_start = start_character;
            }
        }

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    //
    // SIGNATURE HELP
    //

    fn signature_help(
        &mut self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>, ResponseError> {
        let uri = params.text_document_position_params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "signature help");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for signature help");
        let Some(help) = self.cancellable(|| {
            EngineIde::new(&self.engine, &self.paths).signature_help(&path, internal_position)
        }) else {
            return Ok(None);
        };

        let active_parameter = help.active_parameter.map(|index| index as u32);
        let parameters = help
            .parameters
            .into_iter()
            .map(|label| ParameterInformation {
                label: ParameterLabel::Simple(label),
                documentation: None,
            })
            .collect();

        let signature_help = SignatureHelp {
            signatures: vec![SignatureInformation {
                label: help.label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter,
            }],
            active_signature: Some(0),
            active_parameter,
        };

        Ok(Some(signature_help))
    }

    //
    // FORMATTING
    //

    fn formatting(
        &mut self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, ResponseError> {
        let uri = params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };

        tracing::debug!(?path, "format");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        };

        let new_text = match format::format(
            document.tree().root_node(),
            document.rope(),
            self.config.format,
        ) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return Ok(None);
            }
        };

        let rope = document.rope();
        let last_line = rope.len_lines().saturating_sub(1);
        let document_end = position::internal_position_to_lsp(
            rope,
            self.position_encoding,
            TextPosition {
                line_index: last_line,
                character_index: rope.line(last_line).len_bytes(),
            },
        );
        let edits = vec![TextEdit {
            new_text,
            range: Range::new(Position::new(0, 0), document_end),
        }];

        Ok(Some(edits))
    }

    fn range_formatting(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, ResponseError> {
        let uri = params.text_document.uri;
        let range = params.range;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };

        tracing::debug!(?path, "format");

        let Some(document) = self.opened_document(&path) else {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        };

        let internal_range = self
            .to_internal_range(&path, range)
            .expect("opened document rope available for range formatting");
        let Some(node) = document.tree().root_node().descendant_for_point_range(
            Point::new(
                internal_range.start.line_index,
                internal_range.start.character_index,
            ),
            Point::new(
                internal_range.end.line_index,
                internal_range.end.character_index,
            ),
        ) else {
            tracing::info!(?range, "no node for range");
            return Ok(None);
        };

        let new_text = match format::format(node, document.rope(), self.config.format) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(?error, "failed to format");
                return Ok(None);
            }
        };

        let edits = vec![TextEdit {
            new_text,
            range: position::tree_sitter_range_to_lsp(
                document.rope(),
                self.position_encoding,
                node.range(),
            ),
        }];

        Ok(Some(edits))
    }

    //
    // REFERENCES
    //

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>, ResponseError> {
        let uri = params.text_document_position.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        tracing::debug!(?path, ?position, ?include_declaration, "find references");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for references");
        let references = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).references(
                    &path,
                    internal_position,
                    include_declaration,
                )
            })
            .map(|locations| {
                locations
                    .into_iter()
                    .map(|location| self.convert_location(location))
                    .collect()
            });

        Ok(references)
    }

    //
    // RENAME
    //

    fn rename(&mut self, params: RenameParams) -> Result<Option<WorkspaceEdit>, ResponseError> {
        let uri = params.text_document_position.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        tracing::debug!(?path, ?position, ?new_name, "rename");

        if self.opened_document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for rename");
        let workspace_edit = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).rename(
                    &path,
                    internal_position,
                    &new_name,
                )
            })
            .map(|rename_result| {
                let changes = rename_result
                    .edits
                    .into_iter()
                    .map(|(edit_path, edits)| {
                        let uri = Url::from_file_path(&edit_path)
                            .expect("rename edit path should convert to URI");
                        let edits = edits
                            .into_iter()
                            .map(|edit| TextEdit {
                                range: self.to_lsp_range_in(&edit_path, edit.range),
                                new_text: edit.replacement_text,
                            })
                            .collect();
                        (uri, edits)
                    })
                    .collect();

                WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }
            });

        Ok(workspace_edit)
    }

    //
    // SYMBOLS
    //

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>, ResponseError> {
        let uri = params.text_document.uri;
        let Ok(path) = uri.to_file_path() else {
            return Ok(None);
        };

        let Some(file) = self.file_ids.get(&path).copied() else {
            tracing::error!(?path, "symbols not found");
            return Err(path_not_found_error(&path));
        };
        // Document symbols are requested on every keystroke and only need top-level symbols, so
        // they read the engine's parsed tree directly rather than running any analysis phase.
        let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
        let items = index::index(parsed.0.tree().root_node(), parsed.0.rope(), false, false);
        let symbols: Vec<DocumentSymbol> =
            symbols::document(&items, &|range| self.to_lsp_range_in(&path, range));

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>, ResponseError> {
        let query = params.query;

        tracing::debug!(?query);

        let workspace_items = self.cancellable(|| self.package_items_map());
        let symbols = symbols::workspace(&query, &workspace_items, &|path, range| {
            self.to_lsp_range_in(path, range)
        });

        Ok(Some(WorkspaceSymbolResponse::Nested(symbols)))
    }
}

impl EngineWorker {
    // The worker thread's run loop: own the engine, process jobs serially. Each job runs under
    // `catch_unwind` because no async-lsp layer covers this thread — a non-`Cancelled` panic is a
    // coherence failure that must become deterministic process death (a silently-dead worker would leave
    // a zombie server that errors every request and drops every edit).
    fn run(mut self, receiver: mpsc::Receiver<Job>) {
        while let Ok(job) = receiver.recv() {
            let outcome = catch_unwind(AssertUnwindSafe(|| match job {
                Job::Read(closure) => {
                    self.cancel.store(false, Ordering::Relaxed);
                    closure(&mut self);
                }
                Job::Write(closure) => closure(&mut self),
            }));
            if outcome.is_err() {
                // A read closure catches its own `Cancelled` inside `with_cancellation`, which restores the
                // engine's transient state before returning, so `Cancelled` never escapes a job. Anything
                // reaching here is therefore a coherence panic (the hook already printed it) — OR, were the
                // cancellation invariant ever broken, an escaped `Cancelled` whose engine transient state is
                // now uncleaned. Both are incoherent states, so terminate deterministically rather than
                // resume the loop on a corrupted engine.
                tracing::error!("engine worker thread panicked; terminating the language server");
                std::process::exit(1);
            }
        }
    }
}

// The async-lsp frontend: stateless except the channel to the worker, the shared cancellation token, and
// a client handle for the rare error message. Every handler serializes its params into a `Job` and (for
// requests) awaits the worker's reply over a oneshot — never touching the `!Send` engine directly.
struct ServerState {
    sender: mpsc::Sender<Job>,
    cancel: Arc<AtomicBool>,
}

impl ServerState {
    // A cancellable query: the worker resets the token then runs `build`; an edit arriving mid-read flips
    // the token and abandons it (the handler returns its empty default).
    fn read<T, F>(&self, build: F) -> BoxFuture<'static, Result<T, ResponseError>>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineWorker) -> Result<T, ResponseError> + Send + 'static,
    {
        let (reply, receive) = oneshot::channel();
        let job = Job::Read(Box::new(move |worker| {
            let _ = reply.send(build(worker));
        }));
        if self.sender.send(job).is_err() {
            return box_future(Err(worker_gone_error()));
        }
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(worker_gone_error())) })
    }

    // A mutating request (`initialize`): runs to completion (non-cancellable) but still replies.
    fn write_request<T, F>(&self, build: F) -> BoxFuture<'static, Result<T, ResponseError>>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineWorker) -> Result<T, ResponseError> + Send + 'static,
    {
        let (reply, receive) = oneshot::channel();
        let job = Job::Write(Box::new(move |worker| {
            let _ = reply.send(build(worker));
        }));
        if self.sender.send(job).is_err() {
            return box_future(Err(worker_gone_error()));
        }
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(worker_gone_error())) })
    }

    // An input-mutating notification: flip the token (abandon any in-flight read) BEFORE enqueuing the
    // edit, so the in-flight read observes it and the worker processes the edit next.
    fn notify_edit<F>(&self, build: F) -> ControlFlow<async_lsp::Result<()>>
    where
        F: FnOnce(&mut EngineWorker) + Send + 'static,
    {
        self.cancel.store(true, Ordering::Relaxed);
        // These notifications carry document-sync edits (`did_open`/`did_change`/`did_save`). A dead
        // worker channel means the edit cannot be applied, so analysis state would silently diverge from
        // the document; there is no response channel to report the failure on. A desynced analysis state
        // is unrecoverable, so fail loudly rather than serve stale results.
        if self.sender.send(Job::Write(Box::new(build))).is_err() {
            panic!("analysis worker is gone; cannot apply a document-sync edit");
        }
        ControlFlow::Continue(())
    }

    // A notification that does not mutate engine inputs (no token flip needed).
    fn notify<F>(&self, build: F) -> ControlFlow<async_lsp::Result<()>>
    where
        F: FnOnce(&mut EngineWorker) + Send + 'static,
    {
        // As with `notify_edit`, a dead worker channel drops a state mutation with no way to report it,
        // leaving analysis state incoherent; fail loudly rather than continue in a corrupt state.
        if self.sender.send(Job::Write(Box::new(build))).is_err() {
            panic!("analysis worker is gone; cannot apply a state mutation");
        }
        ControlFlow::Continue(())
    }
}

impl LanguageServer for ServerState {
    type Error = ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn initialize(
        &mut self,
        params: InitializeParams,
    ) -> BoxFuture<'static, Result<InitializeResult, ResponseError>> {
        self.write_request(move |worker| worker.initialize(params))
    }

    fn initialized(&mut self, params: InitializedParams) -> ControlFlow<async_lsp::Result<()>> {
        self.notify(move |worker| worker.initialized(params))
    }

    fn did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify_edit(move |worker| worker.did_open(params))
    }

    fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify_edit(move |worker| worker.did_change(params))
    }

    fn did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify_edit(move |worker| worker.did_close(params))
    }

    fn did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify_edit(move |worker| worker.did_save(params))
    }

    fn did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify_edit(move |worker| worker.did_change_watched_files(params))
    }

    fn did_change_configuration(
        &mut self,
        params: DidChangeConfigurationParams,
    ) -> ControlFlow<async_lsp::Result<()>> {
        self.notify(move |worker| worker.did_change_configuration(params))
    }

    fn document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> BoxFuture<'static, Result<DocumentDiagnosticReportResult, ResponseError>> {
        self.read(move |worker| worker.document_diagnostic(params))
    }

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> BoxFuture<'static, Result<Option<CompletionResponse>, ResponseError>> {
        self.read(move |worker| worker.completion(params))
    }

    fn definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoDefinitionResponse>, ResponseError>> {
        self.read(move |worker| worker.definition(params))
    }

    fn hover(
        &mut self,
        params: HoverParams,
    ) -> BoxFuture<'static, Result<Option<Hover>, ResponseError>> {
        self.read(move |worker| worker.hover(params))
    }

    fn inlay_hint(
        &mut self,
        params: InlayHintParams,
    ) -> BoxFuture<'static, Result<Option<Vec<InlayHint>>, ResponseError>> {
        self.read(move |worker| worker.inlay_hint(params))
    }

    fn semantic_tokens_full(
        &mut self,
        params: SemanticTokensParams,
    ) -> BoxFuture<'static, Result<Option<SemanticTokensResult>, ResponseError>> {
        self.read(move |worker| worker.semantic_tokens_full(params))
    }

    fn signature_help(
        &mut self,
        params: SignatureHelpParams,
    ) -> BoxFuture<'static, Result<Option<SignatureHelp>, ResponseError>> {
        self.read(move |worker| worker.signature_help(params))
    }

    fn formatting(
        &mut self,
        params: DocumentFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<TextEdit>>, ResponseError>> {
        self.read(move |worker| worker.formatting(params))
    }

    fn range_formatting(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> BoxFuture<'static, Result<Option<Vec<TextEdit>>, ResponseError>> {
        self.read(move |worker| worker.range_formatting(params))
    }

    fn references(
        &mut self,
        params: ReferenceParams,
    ) -> BoxFuture<'static, Result<Option<Vec<Location>>, ResponseError>> {
        self.read(move |worker| worker.references(params))
    }

    fn rename(
        &mut self,
        params: RenameParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceEdit>, ResponseError>> {
        self.read(move |worker| worker.rename(params))
    }

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> BoxFuture<'static, Result<Option<DocumentSymbolResponse>, ResponseError>> {
        self.read(move |worker| worker.document_symbol(params))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> BoxFuture<'static, Result<Option<WorkspaceSymbolResponse>, ResponseError>> {
        self.read(move |worker| worker.symbol(params))
    }
}

// The worker thread is gone (it `exit(1)`s on a coherence panic, so this is only reachable in the tiny
// window before the process dies): answer the in-flight request with an internal error rather than hang.
fn worker_gone_error() -> ResponseError {
    ResponseError::new(ErrorCode::INTERNAL_ERROR, "engine worker unavailable")
}

// Installed once at startup. The engine's cooperative cancellation raises a `Cancelled` panic that runs
// the process hook before `with_cancellation` catches it, so suppress that payload (per-keystroke, not a
// fault); every other panic prints via the default hook. This hook only PRINTS — process death on a
// worker coherence panic is the worker loop's `exit(1)`, not here (aborting here would kill the process
// on every routine cancellation and defeat the frontend's `CatchUnwindLayer`).
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info.payload().is::<Cancelled>() {
            return;
        }
        default_hook(info);
    }));
}

#[inline(always)]
fn box_future<T: Send + 'static>(content: T) -> BoxFuture<'static, T> {
    Box::pin(async { content })
}

fn empty_full_diagnostic_report() -> DocumentDiagnosticReportResult {
    DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
        RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: None,
                items: Vec::new(),
            },
        },
    ))
}

// A content hash of the report, used as the pull `resultId`: a later pull carrying this id can be
// answered with `Unchanged` whenever the recomputed diagnostics hash to the same value.
fn diagnostics_result_id(items: &[Diagnostic]) -> String {
    let serialized =
        serde_json::to_vec(items).expect("lsp diagnostics are serializable for result-id hashing");
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// The semantic-token legend: the token types the server emits, in the order their indices reference.
// The type notation maps onto four standard token types (there are no modifiers).
fn semantic_token_legend() -> Vec<SemanticTokenType> {
    vec![
        SemanticTokenType::TYPE,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::OPERATOR,
    ]
}

// The legend index for a type-notation role. The `:` separator, the `->` arrow, and the `...` variadic
// marker are all punctuation, so they share the `operator` type.
fn semantic_token_index(role: analysis::TypeTokenRole) -> u32 {
    match role {
        analysis::TypeTokenRole::TypeName => 0,
        analysis::TypeTokenRole::TypeParameter => 1,
        analysis::TypeTokenRole::ParameterName => 2,
        analysis::TypeTokenRole::Separator
        | analysis::TypeTokenRole::Operator
        | analysis::TypeTokenRole::Variadic => 3,
    }
}

// Collects every comment node in the tree in document order, so their `#:` annotations can be
// highlighted. Comments can appear at the top level or nested inside expressions, so the whole tree is
// traversed rather than only the top-level children.
fn collect_comment_nodes<'tree>(
    node: tree_sitter::Node<'tree>,
    comment_nodes: &mut Vec<tree_sitter::Node<'tree>>,
) {
    if node.kind_id() == analysis::tree::kind::COMMENT {
        comment_nodes.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comment_nodes(child, comment_nodes);
    }
}

fn path_not_found_error(path: &Path) -> ResponseError {
    ResponseError::new(
        ErrorCode::REQUEST_FAILED,
        format!("path not found '{}'", path.display()),
    )
}
