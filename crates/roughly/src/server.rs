use {
    crate::{
        config::{self, CONFIG_FILE_NAME, Config, ExperimentalFeatures},
        diagnostics, format,
        index::{self, IndexError, Item},
        lsp_types::{
            CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
            CodeActionProviderCapability, CodeActionResponse, Command, CompletionItem,
            CompletionItemKind, CompletionItemLabelDetails, CompletionList, CompletionOptions,
            CompletionParams, CompletionResponse, Diagnostic, DiagnosticOptions,
            DiagnosticServerCancellationData, DiagnosticServerCapabilities, DiagnosticSeverity,
            DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
            DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
            DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentDiagnosticParams,
            DocumentDiagnosticReport, DocumentDiagnosticReportResult, DocumentFormattingParams,
            DocumentHighlight, DocumentHighlightParams, DocumentRangeFormattingParams,
            DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, Documentation,
            FileChangeType, FileSystemWatcher, FoldingRange, FoldingRangeKind, FoldingRangeParams,
            FoldingRangeProviderCapability, FullDocumentDiagnosticReport, GlobPattern, Hover,
            HoverContents, HoverParams, HoverProviderCapability, InitializeParams,
            InitializeResult, InitializedParams, InlayHint, InlayHintKind, InlayHintLabel,
            InlayHintParams, InsertTextFormat, Location, MarkupContent, MarkupKind, MessageType,
            NumberOrString, OneOf, ParameterInformation, ParameterLabel, Position,
            PublishDiagnosticsParams, Range, ReferenceParams, Registration, RegistrationParams,
            RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
            RelativePattern, RenameParams, SaveOptions, SemanticToken, SemanticTokenType,
            SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
            SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
            ServerCapabilities, ServerInfo, ShowMessageParams, SignatureHelp, SignatureHelpOptions,
            SignatureHelpParams, SignatureInformation, SymbolKind, TextDocumentSyncCapability,
            TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
            TypeDefinitionProviderCapability, UnchangedDocumentDiagnosticReport, Url,
            WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
            notification::{DidChangeWatchedFiles, Notification},
            request::{GotoTypeDefinitionParams, GotoTypeDefinitionResponse},
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

// #[tokio::main] # TODO: understand if this makes a difference???
#[tokio::main(flavor = "current_thread")]
pub async fn run(experimental_features: ExperimentalFeatures) {
    install_panic_hook();
    let runtime = tokio::runtime::Handle::current();

    let (server, _) = async_lsp::MainLoop::new_server(|client| {
        // The `!Send` engine lives on a dedicated worker thread, built INSIDE the thread (it cannot be
        // moved across threads) — and only once `initialize` arrives, because the client-announced
        // workspace root drives config discovery and stub loading. The frontend forwards every LSP op
        // to it over the channel + shares the cancellation token.
        let (sender, receiver) = mpsc::channel::<Job>();
        let cancel = Arc::new(AtomicBool::new(false));
        let seed = WorkerSeed {
            client: client.clone(),
            experimental_features,
            cancel: cancel.clone(),
            runtime: runtime.clone(),
        };
        std::thread::Builder::new()
            .name("roughly-engine".to_owned())
            .spawn(move || run_worker(seed, receiver))
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
    // A config-discovery failure found during `initialize`, surfaced to the client via
    // `window/showMessage` once `initialized` arrives (the safe point to message every client).
    pending_config_error: Option<config::ConfigError>,
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
    // Open `.Rtypes` stub documents, served entirely server-side: the stub corpus is a set-once
    // engine input, so live stub edits publish parse diagnostics from their own buffer and never
    // route through the engine's document inputs.
    stub_documents: HashMap<PathBuf, ropey::Rope>,
    // Whether the one-time "configuration lives in roughly.toml" notice was already shown for an
    // editor-settings payload the server does not apply.
    warned_about_editor_configuration: bool,
    // Memoized per-file symbol items for workspace-symbol search; entries are dropped at the input
    // write paths, so the cache can never serve a stale tree.
    symbol_items_cache: HashMap<PathBuf, Vec<Item>>,
    // Input writes since the last stale-memo sweep; see `maybe_evict_stale_memos`.
    input_writes_since_eviction: u32,
    // Open documents with non-`file:` URIs (untitled buffers, virtual editor documents) are served
    // as standalone script documents, keyed internally by a synthetic path (`document_path`) so the
    // path-keyed host and engine tables can track them. This maps each synthetic path back to the
    // client's URI for outgoing locations and published diagnostics; entries live exactly as long
    // as the document is tracked (inserted on open, removed by `retract_source_input`).
    virtual_document_uris: HashMap<PathBuf, Url>,
    parser: tree_sitter::Parser,
    position_encoding: PositionEncoding,
    // When the client advertises pull-diagnostics support it owns the request cadence, so the
    // server stops pushing `publish_diagnostics` and answers `textDocument/diagnostic` instead.
    client_supports_pull_diagnostics: bool,
    // When a pull client also supports `workspace/diagnostic/refresh`, a package-visible save that
    // moves diagnostics in OTHER documents asks the client to re-pull. Push is suppressed for pull
    // clients, so without this their non-visible dependents would go stale after such a save.
    client_supports_diagnostic_refresh: bool,
    // When the client can process `[start, end)` parameter-label offsets, signature help sends them
    // (unambiguous even when two parameters render to the same text); otherwise it falls back to
    // the parameter's label substring, which such clients locate in the signature label themselves.
    client_supports_parameter_label_offsets: bool,
    // When the client can expand snippet syntax, function completions insert a call — `name($0)`
    // with the cursor between the parens (or `name()$0` past them for a zero-argument function) —
    // instead of the bare name.
    client_supports_snippets: bool,
}

// One LSP operation handed to the worker. `Initialize` is special: it CONSTRUCTS the worker state
// from the client's params (the workspace root drives config discovery and stub loading), so it
// cannot be a closure over `&mut EngineWorker`. A `Read` (a query handler) resets the cancellation
// token at the start so a newer edit can abandon it; a `Write` (a lifecycle/edit notification) runs
// to completion. Both carry a boxed closure that runs the migrated handler on the worker's
// `&mut EngineWorker` and (for requests) sends the reply back over a oneshot.
enum Job {
    Initialize(
        Box<InitializeParams>,
        oneshot::Sender<Result<InitializeResult, ResponseError>>,
    ),
    Read(Box<dyn FnOnce(&mut EngineWorker) + Send>),
    Write(Box<dyn FnOnce(&mut EngineWorker) + Send>),
}

// Everything the worker owns before `initialize` arrives. The full `EngineWorker` exists only after
// `initialize`, so this seed is what the spawned thread starts from.
struct WorkerSeed {
    client: ClientSocket,
    experimental_features: ExperimentalFeatures,
    cancel: Arc<AtomicBool>,
    runtime: tokio::runtime::Handle,
}

impl EngineWorker {
    // Constructed on the worker thread by the `Job::Initialize` handler — not at spawn — because the
    // client's `initialize` params carry the workspace root, and the root drives config discovery,
    // project-stub loading (folded into the engine at construction), and the package scan. The
    // engine is `!Send`, so this still runs on the worker thread itself.
    fn initialize(seed: WorkerSeed, params: InitializeParams) -> (Self, InitializeResult) {
        let WorkerSeed {
            client,
            experimental_features,
            cancel,
            runtime,
        } = seed;

        tracing::info!(?experimental_features, "initialize");

        let workspace_root = workspace_root_from(&params);
        tracing::info!(?workspace_root, "workspace root");

        // A malformed config must not take the server down: analysis works without it, so fall back
        // to the defaults and surface the error once the client is ready for messages.
        let (config, pending_config_error) = match Config::discover(&workspace_root) {
            Ok(config) => (config, None),
            Err(error) => (Config::default(), Some(error)),
        };

        let client_encodings = params
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_deref());
        let position_encoding = PositionEncoding::negotiate(client_encodings);
        tracing::info!(?position_encoding, "negotiated position encoding");

        let client_supports_pull_diagnostics = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.diagnostic.as_ref())
            .is_some();
        let client_supports_diagnostic_refresh = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.diagnostic.as_ref())
            .and_then(|diagnostic| diagnostic.refresh_support)
            .unwrap_or(false);
        tracing::info!(
            pull_diagnostics = client_supports_pull_diagnostics,
            diagnostic_refresh = client_supports_diagnostic_refresh,
            "negotiated diagnostics delivery"
        );

        let client_supports_parameter_label_offsets = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.signature_help.as_ref())
            .and_then(|signature_help| signature_help.signature_information.as_ref())
            .and_then(|signature_information| signature_information.parameter_information.as_ref())
            .and_then(|parameter_information| parameter_information.label_offset_support)
            .unwrap_or(false);

        let client_supports_snippets = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|completion_item| completion_item.snippet_support)
            .unwrap_or(false);

        // A project may ship its own `.Rtypes` stubs under `<root>/stubs/` to override or extend the
        // shipped standard-library corpus. They are read once here and folded into the engine's set-once
        // stub library; they are never re-read on an edit. An unreadable override cannot block the
        // server, so it is logged and skipped (`roughly check` is the surface that reports it).
        let project_stubs = analysis::stdlib::discover_project_stubs(&workspace_root);
        for (path, error_message) in &project_stubs.unreadable {
            tracing::warn!(?path, %error_message, "skipping unreadable project stub override");
        }
        let project_stub_sources = project_stubs
            .sources
            .into_iter()
            .map(|stub_source| stub_source.source)
            .collect();

        let mut worker = Self {
            client,
            cancel,
            runtime,
            config,
            pending_config_error,
            experimental_features,
            workspace_root: workspace_root.clone(),
            open_documents: HashSet::new(),
            engine: Engine::new(RoughlyQueries::with_project_stubs(project_stub_sources)),
            paths: PathTable::new(workspace_root),
            file_ids: HashMap::new(),
            next_file_id: 0,
            documents: HashMap::new(),
            stub_documents: HashMap::new(),
            warned_about_editor_configuration: false,
            symbol_items_cache: HashMap::new(),
            input_writes_since_eviction: 0,
            virtual_document_uris: HashMap::new(),
            parser: analysis::tree::new_parser().expect("server parser should initialize"),
            position_encoding,
            client_supports_pull_diagnostics,
            client_supports_diagnostic_refresh,
            client_supports_parameter_label_offsets,
            client_supports_snippets,
        };

        let workspace_r_path = worker.workspace_r_path();
        if workspace_r_path.is_dir() {
            match index::source_file_paths(&workspace_r_path) {
                Ok(paths) => {
                    for path in paths {
                        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                            panic!("failed to read package source {}: {error}", path.display())
                        });
                        worker.set_source_input(&path, text, true);
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

        worker.rebuild_project_files();
        worker.engine.set_input(Key::Config, worker.engine_config());

        let result = initialize_result(worker.position_encoding, worker.experimental_features);
        (worker, result)
    }

    fn workspace_r_path(&self) -> PathBuf {
        self.workspace_root.join("R")
    }

    // The worker's internal document key for a client URI. `file:` URIs use their filesystem path;
    // any other scheme (an untitled buffer, a virtual editor document) has no filesystem identity,
    // so it cannot join package analysis, but it is still served as a standalone script document
    // keyed by a synthetic path. `None` only for a `file:` URI whose path cannot be extracted —
    // malformed client input, which callers drop with a warning rather than treat as incoherence.
    fn document_path(&self, uri: &Url) -> Option<PathBuf> {
        if uri.scheme() == "file" {
            return uri.to_file_path().ok();
        }
        Some(self.virtual_document_path(uri))
    }

    // The synthetic path for a non-`file:` document: the URI encoded as a single path component
    // under a reserved directory name. It is a pure table key — nothing ever reads it from disk
    // (`is_package_path` is false for it, so `did_close` retracts instead of re-reading). The
    // encoding is deterministic so reopening the same URI reuses the same `FileId`, and escapes
    // path separators (plus `%` for injectivity) so distinct URIs cannot produce the same key.
    fn virtual_document_path(&self, uri: &Url) -> PathBuf {
        let mut encoded = String::with_capacity(uri.as_str().len());
        for character in uri.as_str().chars() {
            match character {
                '%' => encoded.push_str("%25"),
                '/' => encoded.push_str("%2F"),
                '\\' => encoded.push_str("%5C"),
                _ => encoded.push(character),
            }
        }
        self.workspace_root.join(".roughly-virtual").join(encoded)
    }

    // The client-facing URI for a tracked document path — the inverse of `document_path`. Every
    // tracked path is either a virtual document (whose URI is recorded) or a real absolute path,
    // so a failure here means the host tables are incoherent.
    fn document_uri(&self, path: &Path) -> Url {
        match self.virtual_document_uris.get(path) {
            Some(uri) => uri.clone(),
            None => Url::from_file_path(path).unwrap_or_else(|()| {
                panic!("tracked path should convert to a URI: {}", path.display())
            }),
        }
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
    // this returns `Err(Cancelled)` rather than spending a full pass on a result the next edit already
    // superseded — latest-edit-wins. Each caller decides what a cancelled read means for its client:
    // best-effort lookups (hover, completion, ...) answer empty via `unwrap_or_default`, but answers the
    // client treats as authoritative or applies as a mutation (pull diagnostics, rename edits) must
    // return a retryable protocol error instead — an empty answer there would be cached or silently
    // applied as "nothing". Only used by read handlers; edit handlers run their (different-file-
    // publishing) diagnostics to completion via the plain `fetch` path.
    fn cancellable<R>(&self, body: impl FnOnce() -> R) -> Result<R, Cancelled> {
        self.engine.with_cancellation(self.cancel.clone(), body)
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
    // the request, so the outgoing range is encoded against that document's rope. The fallback for
    // an untracked path passes byte columns through as code units — wrong for non-ASCII lines under
    // UTF-16 — so it warns: reaching it means a range was produced for a document the engine does
    // not hold, which is worth surfacing even though the degraded range is usually still usable.
    fn to_lsp_range_in(&self, path: &Path, range: TextRange) -> Range {
        match self.parsed_for(path) {
            Some(parsed) => {
                position::internal_range_to_lsp(parsed.0.rope(), self.position_encoding, range)
            }
            None => {
                tracing::warn!(
                    ?path,
                    "encoding a range for an untracked document; byte columns passed through as code units"
                );
                Range::new(
                    Position::new(
                        range.start.line_index as u32,
                        range.start.character_index as u32,
                    ),
                    Position::new(
                        range.end.line_index as u32,
                        range.end.character_index as u32,
                    ),
                )
            }
        }
    }

    fn to_lsp_position_in(&self, path: &Path, position: TextPosition) -> Position {
        match self.parsed_for(path) {
            Some(parsed) => position::internal_position_to_lsp(
                parsed.0.rope(),
                self.position_encoding,
                position,
            ),
            None => {
                tracing::warn!(
                    ?path,
                    "encoding a position for an untracked document; byte column passed through as code units"
                );
                Position::new(position.line_index as u32, position.character_index as u32)
            }
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
        if config.check.unused {
            rendered.extend(file_diagnostics.unused.iter().cloned());
        }
        if config.check.typing {
            self.engine.group().with_interner(|interner| {
                for error in &file_diagnostics.type_errors {
                    rendered.push(analysis::Diagnostic::from_inference_error(
                        error, fallback, interner,
                    ));
                }
            });
        }
        if file_diagnostics
            .strict_override
            .unwrap_or(config.check.strict)
        {
            rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
            for diagnostic in &mut rendered {
                diagnostic.escalate_unresolved_to_error();
            }
        }

        let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
        let rendered =
            analysis::diagnostic::apply_suppressions(rendered, &parsed.0.rope().to_string());
        diagnostics::convert_diagnostics(rendered, parsed.0.rope(), self.position_encoding)
    }

    fn convert_location(&self, location: analysis::ide::Location) -> Location {
        Location {
            uri: self.document_uri(&location.path),
            range: self.to_lsp_range_in(&location.path, location.range),
        }
    }

    // Workspace symbols index every package document's tree (the `R/` files). The host already knows which
    // Symbol items per file — package documents plus open scripts — for workspace-symbol search.
    // Indexing walks the whole tree, so items are memoized per file and invalidated at the one
    // input-write chokepoint (`set_parsed_input` / `retract_source_input`): a symbol query between
    // keystrokes re-walks nothing.
    fn package_items_map(&mut self) -> HashMap<PathBuf, Vec<Item>> {
        let r_path = self.workspace_r_path();
        let candidates = self
            .file_ids
            .iter()
            .filter(|(path, _)| path.starts_with(&r_path) || self.open_documents.contains(*path))
            .map(|(path, file)| (path.clone(), *file))
            .collect::<Vec<_>>();
        candidates
            .into_iter()
            .map(|(path, file)| {
                if let Some(items) = self.symbol_items_cache.get(&path) {
                    return (path, items.clone());
                }
                let parsed = self.engine.fetch::<ParsedDocument>(Key::Parse(file));
                let items =
                    index::index(parsed.0.tree().root_node(), parsed.0.rope(), false, false);
                self.symbol_items_cache.insert(path.clone(), items.clone());
                (path, items)
            })
            .collect()
    }

    fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            check: self.config.check,
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
        let mut parser = analysis::tree::new_parser().expect("server parser should initialize");
        let document = Document::parse(&mut parser, &text)
            .unwrap_or_else(|_| panic!("failed to parse source input {}", path.display()));
        self.set_parsed_input(path, document, is_package);
    }

    // Feeds an already-parsed document into the engine — the one parse per edit. Open buffers pass
    // their incrementally-maintained document here, so a keystroke never re-parses the file.
    fn set_parsed_input(&mut self, path: &Path, document: Document, is_package: bool) {
        self.symbol_items_cache.remove(path);
        let file = self.file_id_for(path);
        self.engine
            .set_input(Key::SourceText(file), ParsedDocument(document));
        self.engine.set_input(
            Key::DocumentKind(file),
            if is_package {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            },
        );
        self.paths.insert(file, path.to_path_buf());
        self.maybe_evict_stale_memos();
    }

    // The memory bound for a long editing session: derived engine keys are minted per file and per
    // symbol, so transient names (a global typed character by character, deleted files) leave memo
    // slots nothing will fetch again. Every `EVICTION_INTERVAL` input writes, drop the slots not read
    // within the last `EVICTION_KEEP_REVISIONS` revisions — anything a feature still uses is
    // revalidated (and so refreshed) far more often than that, so eviction only touches garbage; a
    // false positive merely recomputes on the next fetch.
    fn maybe_evict_stale_memos(&mut self) {
        const EVICTION_INTERVAL: u32 = 1024;
        const EVICTION_KEEP_REVISIONS: u64 = 8192;
        self.input_writes_since_eviction += 1;
        if self.input_writes_since_eviction >= EVICTION_INTERVAL {
            self.input_writes_since_eviction = 0;
            let dropped = self.engine.evict_stale_memos(EVICTION_KEEP_REVISIONS);
            tracing::debug!(dropped, "evicted stale engine memos");
        }
    }

    // Drop a file from the engine (deletion or a closed, no-longer-tracked document): tombstone its source
    // input so dependents revalidate against the smaller set, and remove it from the host tables.
    fn retract_source_input(&mut self, path: &Path) {
        self.symbol_items_cache.remove(path);
        if let Some(file) = self.file_ids.remove(path) {
            self.engine.remove_input(&Key::SourceText(file));
            self.engine.remove_input(&Key::DocumentKind(file));
            self.paths.remove(file);
            self.maybe_evict_stale_memos();
        }
        self.virtual_document_uris.remove(path);
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

    // Publishes a malformed `roughly.toml` as a diagnostic on the config file itself — pointing at
    // the offending key when the parse error carries a span — so it lands in the problems panel and
    // persists until fixed, unlike the one-shot message. `None` clears it (the config loads again).
    fn publish_config_diagnostics(&mut self, error: Option<&config::ConfigError>) {
        let config_path = self.workspace_root.join(CONFIG_FILE_NAME);
        let Ok(uri) = Url::from_file_path(&config_path) else {
            return;
        };
        let diagnostics = error
            .map(|error| {
                let (line, column) = error.parse_location().unwrap_or((1, 1));
                let start = Position::new(
                    line.saturating_sub(1) as u32,
                    column.saturating_sub(1) as u32,
                );
                vec![Diagnostic {
                    range: Range {
                        start,
                        end: Position::new(start.line, start.character + 1),
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("config".to_owned())),
                    code_description: None,
                    source: Some("roughly".into()),
                    message: error.to_string(),
                    related_information: None,
                    tags: None,
                    data: None,
                }]
            })
            .unwrap_or_default();
        if let Err(error) = self
            .client
            .publish_diagnostics(PublishDiagnosticsParams::new(uri, diagnostics, None))
        {
            tracing::error!(?error, "failed to publish config diagnostics");
        }
    }

    // Brings every client-visible document current after a change that can move diagnostics in many
    // documents at once (a save, or a config reload). Precisely detecting which documents moved is
    // not reliably available — cross-file naming diagnostics move even when type checking is off —
    // so conservatively refresh everything; these events are infrequent. Pull clients own the
    // request cadence (push is suppressed for them), so they are asked to re-pull — without that,
    // their non-visible dependents would go stale; push clients get every open document republished
    // from the engine.
    fn refresh_all_diagnostics(&mut self) {
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

        for open_path in self.open_documents.clone() {
            if !self.file_ids.contains_key(&open_path) {
                continue;
            }
            let open_uri = self.document_uri(&open_path);
            let diagnostics = self.convert_document_diagnostics(&open_path);
            if let Err(error) = self
                .client
                .publish_diagnostics(PublishDiagnosticsParams::new(open_uri, diagnostics, None))
            {
                tracing::error!(?error, "failed to publish diagnostics");
            }
        }
    }
}

impl EngineWorker {
    fn initialized(&mut self, _: InitializedParams) {
        if let Some(error) = self.pending_config_error.take() {
            self.report_error(format!("{error}; using the default configuration"));
            self.publish_config_diagnostics(Some(&error));
        }

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
                        ]
                        .into_iter()
                        // The governing config may live in an ancestor ABOVE the workspace root
                        // (discovery walks upward); watch the discovered file's own directory too,
                        // or live edits to it would silently not apply until a restart. The
                        // in-root watcher above stays: a config created at the root later takes
                        // precedence and must also trigger a reload.
                        .chain(
                            Config::discover_path(&self.workspace_root)
                                .filter(|config_path| {
                                    config_path.parent() != Some(self.workspace_root.as_path())
                                })
                                .and_then(|config_path| {
                                    let directory = config_path.parent()?;
                                    Some(FileSystemWatcher {
                                        glob_pattern: GlobPattern::Relative(RelativePattern {
                                            base_uri: OneOf::Right(
                                                Url::from_file_path(directory).ok()?,
                                            ),
                                            pattern: CONFIG_FILE_NAME.into(),
                                        }),
                                        kind: None,
                                    })
                                }),
                        )
                        .collect(),
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
        let Some(path) = self.document_path(&uri) else {
            tracing::warn!(%uri, "ignoring did_open for a file URI without a usable path");
            return;
        };
        if uri.scheme() != "file" {
            self.virtual_document_uris.insert(path.clone(), uri.clone());
        }
        let text = &params.text_document.text;

        tracing::debug!(?path, "did open");

        if is_stub_document(&path) {
            let rope = ropey::Rope::from_str(text);
            self.stub_documents.insert(path, rope.clone());
            if !self.client_supports_pull_diagnostics {
                let diagnostics = stub_document_diagnostics(&rope, self.position_encoding);
                if let Err(error) = self
                    .client
                    .publish_diagnostics(PublishDiagnosticsParams::new(
                        uri,
                        diagnostics,
                        Some(params.text_document.version),
                    ))
                {
                    tracing::error!(?error, "failed to publish stub diagnostics");
                }
            }
            return;
        }

        let document = Document::parse(&mut self.parser, text)
            .unwrap_or_else(|_| panic!("failed to parse open document buffer {}", path.display()));
        self.documents.insert(path.clone(), document.clone());
        self.open_documents.insert(path.clone());
        let is_package = self.is_package_path(&path);
        self.set_parsed_input(&path, document, is_package);
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
        let Some(path) = self.document_path(&uri) else {
            tracing::warn!(%uri, "ignoring did_close for a file URI without a usable path");
            return;
        };

        tracing::debug!(?path, "did close");

        if self.stub_documents.remove(&path).is_some() {
            return;
        }

        self.open_documents.remove(&path);
        self.documents.remove(&path);
        if self.is_package_path(&path) {
            // A closed package file still on disk reverts to its on-disk text (discarding unsaved buffer
            // edits); the file set is unchanged, so no `rebuild_project_files`. The read can race a
            // concurrent delete or rename (close events are not ordered against the filesystem), which
            // is recoverable exactly like the watcher path: treat the file as deleted and let a later
            // watcher event re-sync it if it reappears.
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.set_source_input(&path, text, true);
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        ?path,
                        ?error,
                        "failed to reload package source on close; treating it as deleted"
                    );
                }
            }
        }
        // A deleted package file, a closed script, or a closed virtual document: no longer tracked.
        self.retract_source_input(&path);
        self.rebuild_project_files();
    }

    fn did_change(&mut self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            tracing::warn!(%uri, "ignoring did_change for a file URI without a usable path");
            return;
        };
        let content_changes = params.content_changes;

        tracing::debug!(?path, "did change");

        if let Some(rope) = self.stub_documents.get_mut(&path) {
            for change in content_changes {
                match change.range {
                    None => *rope = ropey::Rope::from_str(&change.text),
                    Some(range) => {
                        let start = position::lsp_position_to_internal(
                            rope,
                            self.position_encoding,
                            range.start,
                        );
                        let end = position::lsp_position_to_internal(
                            rope,
                            self.position_encoding,
                            range.end,
                        );
                        let start_char =
                            rope.line_to_char(start.line_index) + start.character_index;
                        let end_char = rope.line_to_char(end.line_index) + end.character_index;
                        rope.remove(start_char..end_char);
                        rope.insert(start_char, &change.text);
                    }
                }
            }
            if !self.client_supports_pull_diagnostics {
                let rope = self.stub_documents[&path].clone();
                let diagnostics = stub_document_diagnostics(&rope, self.position_encoding);
                if let Err(error) = self
                    .client
                    .publish_diagnostics(PublishDiagnosticsParams::new(
                        uri,
                        diagnostics,
                        Some(params.text_document.version),
                    ))
                {
                    tracing::error!(?error, "failed to publish stub diagnostics");
                }
            }
            return;
        }

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
            // A change without a range replaces the whole document. Clients may legally fall back
            // to full-text sync even though the server negotiated incremental sync.
            let Some(range) = change.range else {
                let document =
                    Document::parse(&mut self.parser, &change.text).unwrap_or_else(|_| {
                        panic!(
                            "failed to parse full-text did_change buffer {}",
                            path.display()
                        )
                    });
                self.documents.insert(path.clone(), document);
                continue;
            };
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
        // A text-only edit leaves the file SET unchanged, so only the file's input is re-set (no
        // `rebuild_project_files`). The incrementally-maintained buffer IS the input — the edit
        // already re-parsed it in place, so the engine sees the same tree without a second parse
        // or a whole-buffer re-materialization.
        let document = self
            .documents
            .get(&path)
            .expect("open document buffer present after did_change")
            .clone();
        let is_package = self.is_package_path(&path);
        self.set_parsed_input(&path, document, is_package);

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
        let Some(path) = self.document_path(&uri) else {
            tracing::warn!(%uri, "ignoring did_save for a file URI without a usable path");
            return;
        };

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

        // A package-visible save can move diagnostics in dependent files, and live edits push only
        // the edited document, so a save refreshes every client-visible document. The engine already
        // reflects the synced text (the edit inputs were set as the changes arrived), so each
        // document's current diagnostics are correct.
        self.refresh_all_diagnostics();
    }

    fn did_change_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        let workspace_r_path = self.workspace_r_path();

        let mut config_changed = false;
        for change in params.changes {
            let uri = change.uri;
            let typ = change.typ;
            // Watched files are filesystem entities by definition; a non-file URI here is client
            // nonsense, not incoherent server state.
            let Ok(path) = uri.to_file_path() else {
                tracing::warn!(%uri, "ignoring watched-file change for a non-file URI");
                continue;
            };

            tracing::info!(?path, ?typ, "watched file changed");

            // Any watched `roughly.toml` — the workspace root's or a governing ancestor's —
            // triggers re-discovery. Matching by file name rather than a full-path comparison also
            // tolerates a symlinked root or a case-normalizing client, where the client's path
            // spelling differs from ours; re-running discovery is idempotent either way.
            if path
                .file_name()
                .is_some_and(|name| name == CONFIG_FILE_NAME)
            {
                // Re-run discovery rather than reading the changed file directly, so a deleted
                // workspace config correctly falls back to an ancestor config or the defaults.
                match Config::discover(&self.workspace_root) {
                    Ok(config) => {
                        config_changed |= config != self.config;
                        self.config = config;
                        self.publish_config_diagnostics(None);
                    }
                    Err(error) => {
                        self.report_error(format!("{error}; keeping the previous configuration"));
                        self.publish_config_diagnostics(Some(&error));
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
        // A config change can move diagnostics in every document (e.g. toggling `[check] strict`),
        // so apply it to client-visible diagnostics immediately rather than on the next edit.
        if config_changed {
            self.refresh_all_diagnostics();
        }
    }

    fn did_change_configuration(&mut self, params: DidChangeConfigurationParams) {
        // Roughly is configured through `roughly.toml`, not editor settings; the notification must
        // still be accepted (Zed sends it unconditionally). Silently discarding a non-empty
        // settings payload would leave the user believing their editor configuration applied, so
        // say where configuration actually lives — once, not per notification.
        if params.settings.is_null() {
            return;
        }
        if self.warned_about_editor_configuration {
            return;
        }
        self.warned_about_editor_configuration = true;
        if let Err(error) = self.client.show_message(ShowMessageParams {
            typ: MessageType::INFO,
            message: "Roughly is configured through roughly.toml in the workspace root; \
                      editor settings sent via workspace/didChangeConfiguration are not applied."
                .to_owned(),
        }) {
            tracing::warn!(?error, "failed to send the configuration notice");
        }
    }

    //
    // PULL DIAGNOSTICS
    //

    fn document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult, ResponseError> {
        let uri = params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(empty_full_diagnostic_report());
        };

        tracing::debug!(?path, "document diagnostic");

        // A stub buffer is not an engine document: its report is the standalone stub parse the push
        // path serves, computed against the live rope. An unopened stub answers empty like any
        // untracked pull.
        if is_stub_document(&path) {
            let Some(rope) = self.stub_documents.get(&path) else {
                return Ok(empty_full_diagnostic_report());
            };
            let items = stub_document_diagnostics(rope, self.position_encoding);
            return Ok(diagnostic_report(
                items,
                params.previous_result_id.as_deref(),
            ));
        }

        // Unlike the sync notifications, a pull can legitimately target a document the server does
        // not track; answer with an empty full report rather than panicking.
        if !self.file_ids.contains_key(&path) {
            tracing::debug!(?path, "pull diagnostic for untracked document");
            return Ok(empty_full_diagnostic_report());
        }

        // The engine computes the report on demand (typecheck included via the `Diagnostics` fetch in
        // `convert_document_diagnostics`), so it already equals the push path and reflects package-visible
        // edits in dependent files — no separate full pass is needed. A pull report is authoritative —
        // the client caches it until the next pull — so a cancelled read must NOT degrade to an empty
        // report ("this document is clean"); per the LSP spec it answers `ServerCancelled` with
        // `retriggerRequest`, so the client pulls again.
        let Ok(items) = self.cancellable(|| self.convert_document_diagnostics(&path)) else {
            return Err(diagnostics_cancelled_error());
        };
        Ok(diagnostic_report(
            items,
            params.previous_result_id.as_deref(),
        ))
    }

    //
    // COMPLETION
    //

    fn completion(
        &mut self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, ResponseError> {
        let Some(path) = self.document_path(&params.text_document_position.text_document.uri)
        else {
            return Ok(None);
        };
        let position = params.text_document_position.position;

        tracing::debug!(?path, "completion");

        if self.document(&path).is_none() {
            tracing::error!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for completion");
        let snippets = self.client_supports_snippets;
        let completions = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).completion(&path, internal_position)
            })
            .unwrap_or_default()
            .map(|result| {
                CompletionResponse::List(CompletionList {
                    is_incomplete: result.is_incomplete,
                    items: result
                        .items
                        .into_iter()
                        .map(|item| {
                            // A snippet-capable client gets a call for a function item: the cursor
                            // lands between the parens when the call takes arguments, past them
                            // when it takes none (unknown signatures get the with-arguments shape).
                            let call_snippet = (snippets
                                && item.kind == analysis::CompletionItemKind::Function)
                                .then(|| {
                                    if item.takes_arguments == Some(false) {
                                        format!("{}()$0", item.label)
                                    } else {
                                        format!("{}($0)", item.label)
                                    }
                                });
                            CompletionItem {
                                label: item.label,
                                label_details: Some(CompletionItemLabelDetails {
                                    detail: None,
                                    description: Some(match item.source {
                                        analysis::CompletionItemSource::Keyword => "Keyword".into(),
                                        analysis::CompletionItemSource::Local => "Local".into(),
                                        analysis::CompletionItemSource::Global => "Global".into(),
                                        analysis::CompletionItemSource::Stdlib => "Stdlib".into(),
                                        analysis::CompletionItemSource::Field => "Field".into(),
                                        analysis::CompletionItemSource::Type => "Type".into(),
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
                                    analysis::CompletionItemKind::Field => {
                                        CompletionItemKind::FIELD
                                    }
                                    analysis::CompletionItemKind::Type => {
                                        CompletionItemKind::STRUCT
                                    }
                                }),
                                detail: item.detail,
                                documentation: item.documentation.map(|documentation| {
                                    Documentation::MarkupContent(MarkupContent {
                                        kind: MarkupKind::Markdown,
                                        value: documentation,
                                    })
                                }),
                                insert_text_format: call_snippet
                                    .is_some()
                                    .then_some(InsertTextFormat::SNIPPET),
                                // Typing continues inside the inserted call, so ask the editor for
                                // parameter hints exactly as if `(` had been typed.
                                command: call_snippet.as_ref().map(|_| Command {
                                    title: "trigger parameter hints".to_owned(),
                                    command: "editor.action.triggerParameterHints".to_owned(),
                                    arguments: None,
                                }),
                                insert_text: call_snippet,
                                ..Default::default()
                            }
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "goto definition");

        // Inside a `.Rtypes` buffer, a type name jumps to its `@type` declaration line in the same
        // file (stub files are self-contained: the shipped corpus is not on disk to jump into).
        if is_stub_document(&path) {
            let Some(rope) = self.stub_documents.get(&path) else {
                return Ok(None);
            };
            let cursor = position::lsp_position_to_internal(rope, self.position_encoding, position);
            let Some(line) = rope.get_line(cursor.line_index) else {
                return Ok(None);
            };
            let line_text = line.to_string();
            let target = analysis::stub::stub_line_tokens(&line_text)
                .into_iter()
                .find(|token| {
                    token.role == analysis::type_syntax::TypeTokenRole::TypeName
                        && token.start <= cursor.character_index
                        && cursor.character_index <= token.end
                })
                .map(|token| line_text[token.start..token.end].to_owned());
            let Some(name) = target else {
                return Ok(None);
            };
            for (line_index, declaration_line) in rope.lines().enumerate() {
                let text = declaration_line.to_string();
                let trimmed = text.trim_start();
                let Some(declared) = trimmed.strip_prefix("@type") else {
                    continue;
                };
                if declared.trim() == name {
                    let column = text.len() - trimmed.len();
                    let start = position::internal_position_to_lsp(
                        rope,
                        self.position_encoding,
                        TextPosition {
                            line_index,
                            character_index: column,
                        },
                    );
                    let end = position::internal_position_to_lsp(
                        rope,
                        self.position_encoding,
                        TextPosition {
                            line_index,
                            character_index: text.trim_end().len(),
                        },
                    );
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: uri.clone(),
                        range: Range { start, end },
                    })));
                }
            }
            return Ok(None);
        }

        if self.document(&path).is_none() {
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
            .unwrap_or_default()
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "hover");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for hover");
        let Some(hover_info) = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).hover(&path, internal_position)
            })
            .unwrap_or_default()
        else {
            tracing::debug!(?position, "hover target not found");
            return Ok(None);
        };

        let value = ide::render_hover_markdown(&hover_info, self.config.debug);

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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };

        tracing::debug!(?path, "inlay hints");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let viewport = self
            .to_internal_range(&path, params.range)
            .expect("opened document rope available for inlay hints");
        let raw_hints = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).inlay_hints(&path, Some(viewport))
            })
            .unwrap_or_default();
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };

        tracing::debug!(?path, "semantic tokens");

        // A `.Rtypes` stub buffer is pure type notation: classify each line with the stub-aware
        // lexer instead of scanning an R tree for `#:` comments.
        if is_stub_document(&path) {
            let Some(rope) = self.stub_documents.get(&path) else {
                return Ok(None);
            };
            let rope = rope.clone();
            let mut previous_line = 0u32;
            let mut previous_start = 0u32;
            let mut data = Vec::new();
            for (line_index, line) in rope.lines().enumerate() {
                let line_text = line.to_string();
                for token in analysis::stub::stub_line_tokens(&line_text) {
                    let start_position = position::internal_position_to_lsp(
                        &rope,
                        self.position_encoding,
                        TextPosition {
                            line_index,
                            character_index: token.start,
                        },
                    );
                    let end_position = position::internal_position_to_lsp(
                        &rope,
                        self.position_encoding,
                        TextPosition {
                            line_index,
                            character_index: token.end,
                        },
                    );
                    let line_number = start_position.line;
                    let start_character = start_position.character;
                    let length = end_position.character.saturating_sub(start_character);
                    if length == 0 {
                        continue;
                    }
                    let delta_line = line_number - previous_line;
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
                    previous_line = line_number;
                    previous_start = start_character;
                }
            }
            return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data,
            })));
        }

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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "signature help");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for signature help");
        let Some(help) = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).signature_help(&path, internal_position)
            })
            .unwrap_or_default()
        else {
            return Ok(None);
        };

        let signatures = help
            .signatures
            .into_iter()
            .map(|signature| {
                let active_parameter = signature.active_parameter.map(|index| index as u32);
                let parameters = signature
                    .parameters
                    .iter()
                    .map(|span| {
                        let text = signature
                            .label
                            .get(span.clone())
                            .expect("parameter span should lie within the signature label");
                        // Label offsets are UTF-16 code units per the LSP spec (independent of the
                        // negotiated document position encoding, which governs only document positions).
                        let label = if self.client_supports_parameter_label_offsets {
                            let prefix = signature.label.get(..span.start).expect(
                                "parameter span should start on a label character boundary",
                            );
                            let start = utf16_length(prefix);
                            let end = start + utf16_length(text);
                            ParameterLabel::LabelOffsets([start, end])
                        } else {
                            ParameterLabel::Simple(text.to_owned())
                        };
                        ParameterInformation {
                            label,
                            documentation: None,
                        }
                    })
                    .collect();
                SignatureInformation {
                    label: signature.label,
                    documentation: None,
                    parameters: Some(parameters),
                    active_parameter,
                }
            })
            .collect::<Vec<_>>();

        // The top-level active parameter mirrors the active signature's own; clients that ignore
        // per-signature values still highlight correctly.
        let active_parameter = signatures
            .get(help.active_signature)
            .and_then(|signature| signature.active_parameter);
        let signature_help = SignatureHelp {
            signatures,
            active_signature: Some(help.active_signature as u32),
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };

        tracing::debug!(?path, "format");

        let Some(document) = self.document(&path) else {
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };

        tracing::debug!(?path, "format");

        let Some(document) = self.document(&path) else {
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
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        tracing::debug!(?path, ?position, ?include_declaration, "find references");

        if self.document(&path).is_none() {
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
            .unwrap_or_default()
            .map(|locations| {
                locations
                    .into_iter()
                    .map(|location| self.convert_location(location))
                    .collect()
            });

        Ok(references)
    }

    fn document_highlight(
        &mut self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>, ResponseError> {
        let uri = params.text_document_position_params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        if self.document(&path).is_none() {
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for document highlight");
        // Highlights are the current document's slice of the references answer: every read and
        // write of the slot under the cursor, declaration included.
        let highlights = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).references(&path, internal_position, true)
            })
            .unwrap_or_default()
            .map(|locations| {
                locations
                    .into_iter()
                    .filter(|location| location.path == path)
                    .map(|location| DocumentHighlight {
                        range: self.to_lsp_range_in(&location.path, location.range),
                        kind: None,
                    })
                    .collect()
            });

        Ok(highlights)
    }

    fn folding_range(
        &mut self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>, ResponseError> {
        let uri = params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let Some(document) = self.document(&path) else {
            return Err(path_not_found_error(&path));
        };

        // Fold every multi-line braced/parenthesized region and call-argument list, plus each run
        // of consecutive comment lines (`#:` annotation blocks fold as one region). Walking the raw
        // tree keeps this parse-only — no analysis phase runs for folding.
        let mut ranges = Vec::new();
        let mut cursor = document.tree().walk();
        let mut comment_run: Option<(usize, usize)> = None;
        fn flush_comment_run(run: &mut Option<(usize, usize)>, ranges: &mut Vec<FoldingRange>) {
            if let Some((start, end)) = run.take()
                && end > start
            {
                ranges.push(FoldingRange {
                    start_line: start as u32,
                    end_line: end as u32,
                    kind: Some(FoldingRangeKind::Comment),
                    ..FoldingRange::default()
                });
            }
        }
        let mut done = false;
        while !done {
            let node = cursor.node();
            if node.kind_id() == analysis::tree::kind::COMMENT {
                let row = node.range().start_point.row;
                comment_run = match comment_run {
                    Some((start, end)) if row == end + 1 => Some((start, row)),
                    Some(run) => {
                        flush_comment_run(&mut Some(run), &mut ranges);
                        Some((row, row))
                    }
                    None => Some((row, row)),
                };
            } else if matches!(
                node.kind_id(),
                analysis::tree::kind::BRACED_EXPRESSION
                    | analysis::tree::kind::PARENTHESIZED_EXPRESSION
                    | analysis::tree::kind::ARGUMENTS
            ) {
                let range = node.range();
                if range.end_point.row > range.start_point.row {
                    ranges.push(FoldingRange {
                        start_line: range.start_point.row as u32,
                        // Keep the closing delimiter's line visible.
                        end_line: (range.end_point.row - 1) as u32,
                        kind: Some(FoldingRangeKind::Region),
                        ..FoldingRange::default()
                    });
                }
            }

            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    done = true;
                    break;
                }
            }
        }
        flush_comment_run(&mut comment_run, &mut ranges);

        Ok(Some(ranges))
    }

    //
    // RENAME
    //

    fn rename(&mut self, params: RenameParams) -> Result<Option<WorkspaceEdit>, ResponseError> {
        let uri = params.text_document_position.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        tracing::debug!(?path, ?position, ?new_name, "rename");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        // A rename edit is inserted as plain source text, so a non-syntactic name (`my var`,
        // `1st`, a reserved word) would silently produce broken R everywhere the binding occurs.
        // Reject it here with a message the client shows in the rename box.
        if !is_valid_r_identifier(&new_name) {
            return Err(ResponseError::new(
                ErrorCode::INVALID_PARAMS,
                format!("`{new_name}` is not a valid R identifier"),
            ));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for rename");
        // A rename answer is a mutation the client applies: a cancelled read must not degrade to a
        // `null` edit — the client would report a successful rename that changed nothing. An edit
        // arriving mid-rename also invalidates the request's positions, so answer `ContentModified`
        // (the LSP code for "the result became stale"), which clients handle without applying anything.
        let Ok(rename_result) = self.cancellable(|| {
            EngineIde::new(&self.engine, &self.paths).rename(&path, internal_position, &new_name)
        }) else {
            return Err(ResponseError::new(
                ErrorCode::CONTENT_MODIFIED,
                "rename was cancelled by a concurrent edit",
            ));
        };
        let workspace_edit =
            rename_result.map(|rename_result| self.to_workspace_edit(rename_result.edits));

        Ok(workspace_edit)
    }

    // Encodes a per-path edit set (a rename result or a code action's edits) as an LSP
    // `WorkspaceEdit`, converting each edit's range against its own document's rope.
    fn to_workspace_edit(
        &self,
        edits: std::collections::BTreeMap<PathBuf, Vec<analysis::TextEdit>>,
    ) -> WorkspaceEdit {
        let changes = edits
            .into_iter()
            .map(|(edit_path, edits)| {
                let uri = self.document_uri(&edit_path);
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
    }

    //
    // CODE ACTIONS
    //

    fn code_action(
        &mut self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>, ResponseError> {
        let uri = params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };

        tracing::debug!(?path, "code action");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_range = self
            .to_internal_range(&path, params.range)
            .expect("opened document rope available for code actions");
        let actions = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).code_actions(&path, internal_range)
            })
            .unwrap_or_default();
        if actions.is_empty() {
            return Ok(None);
        }

        let response = actions
            .into_iter()
            .map(|action| {
                CodeActionOrCommand::CodeAction(CodeAction {
                    title: action.title,
                    kind: Some(CodeActionKind::from(action.kind.as_lsp_string().to_owned())),
                    edit: Some(self.to_workspace_edit(action.edits)),
                    ..Default::default()
                })
            })
            .collect();
        Ok(Some(response))
    }

    //
    // TYPE DEFINITION
    //

    fn type_definition(
        &mut self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>, ResponseError> {
        let uri = params.text_document_position_params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
            return Ok(None);
        };
        let position = params.text_document_position_params.position;

        tracing::debug!(?path, ?position, "goto type definition");

        if self.document(&path).is_none() {
            tracing::info!(?path, "document not found");
            return Err(path_not_found_error(&path));
        }

        let internal_position = self
            .to_internal_position(&path, position)
            .expect("opened document rope available for type definition");
        let response = self
            .cancellable(|| {
                EngineIde::new(&self.engine, &self.paths).type_definition(&path, internal_position)
            })
            .unwrap_or_default()
            .map(|locations| {
                let mut locations = locations
                    .into_iter()
                    .map(|location| self.convert_location(location))
                    .collect::<Vec<_>>();
                match locations.len() {
                    1 => GotoTypeDefinitionResponse::Scalar(
                        locations.pop().expect("single type definition location"),
                    ),
                    _ => GotoTypeDefinitionResponse::Array(locations),
                }
            });

        Ok(response)
    }

    //
    // SYMBOLS
    //

    fn document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>, ResponseError> {
        let uri = params.text_document.uri;
        let Some(path) = self.document_path(&uri) else {
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
        let mut symbols: Vec<DocumentSymbol> =
            symbols::document(&items, &|range| self.to_lsp_range_in(&path, range));

        // `@type`/`@alias` declarations live in `#:` comments, invisible to the tree-walking
        // indexer, so they join the outline from the lowered module. Ranges are converted before
        // the interner borrow: `to_lsp_range_in` fetches from the engine, which would conflict
        // with a held interner `Ref`.
        let module = self.engine.fetch::<analysis::Module>(Key::Lower(file));
        let definition_ranges = module
            .definitions
            .iter()
            .map(|definition| {
                position::tree_sitter_range_to_lsp(
                    parsed.0.rope(),
                    self.position_encoding,
                    definition.range,
                )
            })
            .collect::<Vec<_>>();
        self.engine.group().with_interner(|interner| {
            for (definition, range) in module.definitions.iter().zip(definition_ranges) {
                let Some(name) = interner.resolve(definition.definition.name) else {
                    continue;
                };
                #[allow(deprecated)] // `deprecated` is a required field of `DocumentSymbol`
                symbols.push(DocumentSymbol {
                    name: name.to_owned(),
                    detail: Some(definition.definition.kind.directive_name().to_owned()),
                    kind: match definition.definition.kind {
                        analysis::DefinitionKind::Type => SymbolKind::STRUCT,
                        analysis::DefinitionKind::Alias => SymbolKind::INTERFACE,
                    },
                    tags: None,
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: None,
                });
            }
        });
        symbols.sort_by_key(|symbol| (symbol.range.start.line, symbol.range.start.character));

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    fn symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>, ResponseError> {
        let query = params.query;

        tracing::debug!(?query);

        // The per-file walks are memoized, so a query between keystrokes touches only changed
        // files; a concurrent edit invalidates entries rather than racing a long read, so no
        // cancellation wrapper is needed here.
        let workspace_items = self.package_items_map();
        let symbols = symbols::workspace(&query, &workspace_items, &|path, range| {
            self.to_lsp_range_in(path, range)
        });

        Ok(Some(WorkspaceSymbolResponse::Nested(symbols)))
    }
}

// The worker thread's run loop: own the engine, process jobs serially. The first job is
// `Initialize` (the frontend's `LifecycleLayer` rejects any earlier request), which constructs the
// full worker state from the client's params. Each job runs under `catch_unwind` because no
// async-lsp layer covers this thread — a non-`Cancelled` panic is a coherence failure that must
// become deterministic process death (a silently-dead worker would leave a zombie server that
// errors every request and drops every edit).
fn run_worker(seed: WorkerSeed, receiver: mpsc::Receiver<Job>) {
    let mut seed = Some(seed);
    let mut worker: Option<EngineWorker> = None;
    while let Ok(job) = receiver.recv() {
        let outcome = catch_unwind(AssertUnwindSafe(|| match job {
            Job::Initialize(params, reply) => {
                let seed = seed
                    .take()
                    .expect("initialize arrives once (LifecycleLayer rejects a second one)");
                let (state, result) = EngineWorker::initialize(seed, *params);
                worker = Some(state);
                if reply.send(Ok(result)).is_err() {
                    tracing::error!("initialize reply receiver dropped");
                }
            }
            Job::Read(closure) => {
                let worker = worker
                    .as_mut()
                    .expect("requests before initialize are rejected by LifecycleLayer");
                worker.cancel.store(false, Ordering::Relaxed);
                closure(worker);
            }
            // LifecycleLayer rejects pre-initialize requests but lets notifications through. The
            // LSP protocol says a server should drop notifications that arrive before
            // `initialize`, and there is no analysis state yet for a dropped one to corrupt — so
            // drop, don't die. (Post-initialize document-sync failures still panic; that policy is
            // about corrupting live state, which does not exist here.)
            Job::Write(closure) => match worker.as_mut() {
                Some(worker) => closure(worker),
                None => tracing::warn!("dropping a notification that arrived before initialize"),
            },
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
        let (reply, receive) = oneshot::channel();
        if self
            .sender
            .send(Job::Initialize(Box::new(params), reply))
            .is_err()
        {
            return box_future(Err(worker_gone_error()));
        }
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(worker_gone_error())) })
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

    fn type_definition(
        &mut self,
        params: GotoTypeDefinitionParams,
    ) -> BoxFuture<'static, Result<Option<GotoTypeDefinitionResponse>, ResponseError>> {
        self.read(move |worker| worker.type_definition(params))
    }

    fn code_action(
        &mut self,
        params: CodeActionParams,
    ) -> BoxFuture<'static, Result<Option<CodeActionResponse>, ResponseError>> {
        self.read(move |worker| worker.code_action(params))
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

    fn document_highlight(
        &mut self,
        params: DocumentHighlightParams,
    ) -> BoxFuture<'static, Result<Option<Vec<DocumentHighlight>>, ResponseError>> {
        self.read(move |worker| worker.document_highlight(params))
    }

    fn folding_range(
        &mut self,
        params: FoldingRangeParams,
    ) -> BoxFuture<'static, Result<Option<Vec<FoldingRange>>, ResponseError>> {
        self.read(move |worker| worker.folding_range(params))
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

// The workspace root the client announced: the first workspace folder, falling back to the older
// `root_uri` field, and — only for clients that provide neither — the server process's working
// directory. This root drives config discovery, the package `R/` scan, the `roughly.toml` watcher,
// and project stub discovery, so none of them may depend on where the server process was launched.
#[allow(deprecated)] // `root_uri` is deprecated in LSP but is the standard fallback for older clients
fn workspace_root_from(params: &InitializeParams) -> PathBuf {
    params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| folder.uri.to_file_path().ok())
        .or_else(|| {
            params
                .root_uri
                .as_ref()
                .and_then(|uri| uri.to_file_path().ok())
        })
        .unwrap_or_else(|| {
            std::env::current_dir().expect("process working directory should be available")
        })
}

fn initialize_result(
    position_encoding: PositionEncoding,
    experimental_features: ExperimentalFeatures,
) -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(position_encoding.kind()),
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                ..Default::default()
            })),
            completion_provider: Some(CompletionOptions {
                // `"` triggers field-name completion inside a `[["..."]]` string subscript.
                trigger_characters: Some(vec!["$".into(), "@".into(), ":".into(), "\"".into()]),
                ..Default::default()
            }),
            definition_provider: Some(OneOf::Left(true)),
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
                identifier: Some("roughly".into()),
                inter_file_dependencies: true,
                workspace_diagnostics: false,
                work_done_progress_options: Default::default(),
            })),
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: Some(OneOf::Left(
                experimental_features.range_formatting,
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
            document_highlight_provider: Some(OneOf::Left(true)),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            rename_provider: Some(OneOf::Left(true)),
            semantic_tokens_provider: Some(
                SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    legend: SemanticTokensLegend {
                        token_types: semantic_token_legend(),
                        token_modifiers: Vec::new(),
                    },
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    range: None,
                    work_done_progress_options: Default::default(),
                }),
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

// The spec-mandated answer for a diagnostic pull the server abandoned: `ServerCancelled` carrying
// `DiagnosticServerCancellationData { retriggerRequest: true }`, so the client re-pulls instead of
// caching an answer that was never computed.
fn diagnostics_cancelled_error() -> ResponseError {
    ResponseError::new_with_data(
        ErrorCode::SERVER_CANCELLED,
        "diagnostics were cancelled by a concurrent edit",
        serde_json::to_value(DiagnosticServerCancellationData::default())
            .expect("diagnostic cancellation data is serializable"),
    )
}

// Wrap freshly computed diagnostics as a pull answer: `Unchanged` when the report's content hash
// matches the client's previous result id, a `Full` report carrying the new id otherwise. Hashing
// content (not edit versions) keeps the unchanged answer correct even under inter-file dependencies
// where the document's own edit version did not move.
fn diagnostic_report(
    items: Vec<Diagnostic>,
    previous_result_id: Option<&str>,
) -> DocumentDiagnosticReportResult {
    let result_id = diagnostics_result_id(&items);
    if previous_result_id == Some(result_id.as_str()) {
        return DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(
            RelatedUnchangedDocumentDiagnosticReport {
                related_documents: None,
                unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                    result_id,
                },
            },
        ));
    }
    DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
        RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: Some(result_id),
                items,
            },
        },
    ))
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
fn is_stub_document(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "Rtypes")
}

// Diagnostics for one `.Rtypes` stub buffer: the declarations the loader would drop (parse failures
// and unharvestable declarations), each spanning its whole line. The analysis runs against a
// throwaway interner: stub buffers must never touch the engine's shared interner or its inputs (the
// loaded corpus is a set-once ambient input; the live buffer is only a document being edited).
fn stub_document_diagnostics(rope: &ropey::Rope, encoding: PositionEncoding) -> Vec<Diagnostic> {
    let text = rope.to_string();
    let mut interner = analysis::Interner::new();
    let type_definitions = analysis::stdlib::stub_editing_type_definitions(&mut interner, &text);
    analysis::stdlib::stub_source_problems(&mut interner, &text, &type_definitions)
        .iter()
        .map(|problem| diagnostics::convert_stub_problem(problem, rope, encoding))
        .collect()
}

// A syntactic R identifier: letters, digits, `.`, and `_`, not starting with a digit or an
// underscore, `.` not followed by a digit at the start, and not a reserved word. Anything else
// would need backtick quoting, which a rename edit does not insert.
fn is_valid_r_identifier(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "if",
        "else",
        "repeat",
        "while",
        "function",
        "for",
        "in",
        "next",
        "break",
        "TRUE",
        "FALSE",
        "NULL",
        "Inf",
        "NaN",
        "NA",
        "NA_integer_",
        "NA_real_",
        "NA_character_",
        "NA_complex_",
    ];
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let starts_validly = match first {
        '.' => !name
            .chars()
            .nth(1)
            .is_some_and(|second| second.is_ascii_digit()),
        character => character.is_alphabetic(),
    };
    starts_validly
        && characters
            .all(|character| character.is_alphanumeric() || character == '.' || character == '_')
        && !RESERVED.contains(&name)
}

fn semantic_token_legend() -> Vec<SemanticTokenType> {
    vec![
        SemanticTokenType::TYPE,
        SemanticTokenType::TYPE_PARAMETER,
        SemanticTokenType::PARAMETER,
        SemanticTokenType::OPERATOR,
        SemanticTokenType::DECORATOR,
    ]
}

// The legend index for a type-notation role. The `:` separator, the `->` arrow, and the `...` variadic
// marker are all punctuation, so they share the `operator` type; an `@`-directive gets the `decorator`
// type so editors theme it distinctly from type names.
fn semantic_token_index(role: analysis::TypeTokenRole) -> u32 {
    match role {
        analysis::TypeTokenRole::TypeName => 0,
        analysis::TypeTokenRole::TypeParameter => 1,
        analysis::TypeTokenRole::ParameterName => 2,
        analysis::TypeTokenRole::Separator
        | analysis::TypeTokenRole::Operator
        | analysis::TypeTokenRole::Variadic => 3,
        analysis::TypeTokenRole::Directive => 4,
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

fn utf16_length(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the wire shape of a cancelled diagnostic pull: the `ServerCancelled` code and camelCase
    // `retriggerRequest: true` data the spec requires for the client to re-pull. The LSP suite
    // cannot trigger an in-flight cancellation deterministically (it depends on engine checkpoint
    // timing), so the protocol contract is asserted here.
    #[test]
    fn cancelled_pull_answer_matches_the_lsp_contract() {
        let error = diagnostics_cancelled_error();
        assert_eq!(error.code, ErrorCode::SERVER_CANCELLED);
        let data = error.data.expect("cancellation answer carries data");
        assert_eq!(
            data.get("retriggerRequest"),
            Some(&serde_json::Value::Bool(true))
        );
    }
}
