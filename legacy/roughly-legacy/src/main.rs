use {
    clap::{Parser, Subcommand},
    roughly_legacy::cli::{self, CommandError, Outcome, OutputFormat},
    std::{path::PathBuf, process::ExitCode},
    tracing_subscriber::prelude::*,
};

fn main() -> ExitCode {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(
            tracing_tree::HierarchicalLayer::new(2)
                .with_ansi(false)
                .with_targets(true)
                .with_writer(std::io::stderr),
        )
        .init();

    let cli = Cli::parse();
    let experimental_features = cli::parse_experimental_flags(
        &cli.experimental_features
            .as_ref()
            .map(|flags| {
                flags
                    .iter()
                    .flat_map(|flag| flag.split(' '))
                    .collect::<Vec<&str>>()
            })
            .unwrap_or_else(Vec::new),
    );

    // Commands run on a thread with an explicit large stack rather than the main thread: the
    // recursive tree walks (the formatter above all) need more than a constrained `ulimit -s`
    // main stack provides on the deepest trees the grammar produces.
    let command = move || match cli.command {
        Command::Check {
            files,
            output,
            min_severity,
        } => exit_code(cli::check(files.as_deref(), output, min_severity)),
        Command::Fmt {
            files,
            check,
            diff,
            verbose,
        } => exit_code(cli::fmt(files.as_deref(), check, diff, verbose)),
        Command::Server {
            stdio: _,
            verbose: _,
        } => {
            cli::server(experimental_features);
            ExitCode::SUCCESS
        }
        Command::Debug(dev) => match dev {
            Debug::AnalysisStats { path } => exit_code(
                roughly_legacy::stats::analysis_stats(path.as_deref()).map(|()| Outcome::Clean),
            ),
            Debug::Index {
                paths,
                recursive,
                show_items,
            } => exit_code(
                cli::index(paths.as_deref(), recursive, show_items).map(|()| Outcome::Clean),
            ),
            Debug::Ast { path } => exit_code(cli::ast(&path).map(|()| Outcome::Clean)),
        },
    };
    std::thread::Builder::new()
        .name("roughly-command".to_owned())
        .stack_size(roughly_legacy::ANALYSIS_STACK_SIZE)
        .spawn(command)
        .expect("command thread should spawn")
        .join()
        .expect("command thread should not panic")
}

// The exit codes are a documented contract (see the docs getting-started page): 0 clean, 1
// findings (diagnostics, or files a `fmt --check`/`--diff` run would change), 2
// usage/configuration/IO errors. Clap's own usage errors also exit 2.
fn exit_code(result: Result<Outcome, CommandError>) -> ExitCode {
    match result {
        Ok(Outcome::Clean) => ExitCode::SUCCESS,
        Ok(Outcome::Findings) => ExitCode::from(1),
        Err(CommandError) => ExitCode::from(2),
    }
}

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    // #[clap(long, action = clap::ArgAction::HelpLong)]
    // help: Option<bool>,
    /// Ignored ... here only to please VS Code
    #[clap(long, default_value_t = true)]
    stdio: bool,
    /// Enable experimental features (e.g. "all" or "feature-1 feature-2")
    #[clap(long, global = true)]
    experimental_features: Option<Vec<String>>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Lint the given files or directories
    Check {
        /// R files to check
        files: Option<Vec<PathBuf>>,
        /// Diagnostic output format
        #[clap(long, value_enum, default_value = "human")]
        output: OutputFormat,
        /// Only report diagnostics at or above this severity (and only they affect the exit code)
        #[clap(long, value_enum, default_value = "warning")]
        min_severity: cli::MinSeverity,
    },
    /// Run the formatter on the given files or directories
    #[clap(alias = "format")]
    Fmt {
        /// R files to format
        files: Option<Vec<PathBuf>>,
        /// Exit with error if files would be modified without making changes
        #[clap(long, default_value_t = false)]
        check: bool,
        /// Show diff instead of modifying files; exit with error if changes needed
        #[clap(long, default_value_t = false)]
        diff: bool,
        /// Print verbose output
        #[clap(short, long, default_value_t = false)]
        verbose: bool,
    },
    /// Run the language server
    #[clap(alias = "lsp")] // here for backwards compatibility
    Server {
        /// Ignored ... here only to please VS Code
        #[clap(long, default_value_t = true)]
        stdio: bool,
        /// Enable verbose logging (ignored for now)
        #[clap(short, long, default_value_t = false)]
        verbose: bool,
    },
    /// Debugging and development commands
    #[command(subcommand)]
    Debug(Debug),
}

#[derive(Debug, Subcommand)]
enum Debug {
    /// Profile the analysis over a workspace: per-phase timings, slowest files, incremental cost
    AnalysisStats {
        /// Workspace directory (or a file whose package root is used); defaults to `.`
        path: Option<PathBuf>,
    },
    /// Print the syntax tree for the given file
    Ast { path: PathBuf },
    /// Index the given files or directories
    Index {
        /// R files to index
        paths: Option<Vec<PathBuf>>,
        /// Recursively index all sub items
        #[clap(long, default_value_t = false)]
        recursive: bool,
        /// Show indexed items
        #[clap(long, default_value_t = false)]
        show_items: bool,
    },
}
