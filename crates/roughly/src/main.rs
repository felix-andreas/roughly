use {
    clap::{Parser, Subcommand},
    roughly::{
        cli::{self, CheckError, DebugError, FmtError},
        experimental::ExperimentalFeatures,
    },
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
    
    // Parse experimental features and warn about unknown ones
    let (experimental_features, unknown_features) = ExperimentalFeatures::from_strings(&cli.experimental_features);
    for unknown in &unknown_features {
        eprintln!("warning: unknown experimental feature: {}", unknown);
    }
    
    match cli.command {
        Command::Check { files } => match cli::check(files.as_deref(), experimental_features) {
            Ok(()) => ExitCode::SUCCESS,
            Err(CheckError) => ExitCode::FAILURE,
        },
        Command::Fmt { files, check, diff } => match cli::fmt(files.as_deref(), check, diff) {
            Ok(()) => ExitCode::SUCCESS,
            Err(FmtError) => ExitCode::FAILURE,
        },
        Command::Server { stdio: _stdio } => {
            cli::server(experimental_features);
            ExitCode::SUCCESS
        }
        Command::Debug(dev) => match dev {
            Debug::Index {
                paths: files,
                recursive,
                show_items,
            } => match cli::index(files.as_deref(), recursive, show_items) {
                Ok(()) => ExitCode::SUCCESS,
                Err(DebugError) => ExitCode::FAILURE,
            },
            Debug::Ast { path } => match cli::ast(&path) {
                Ok(()) => ExitCode::SUCCESS,
                Err(DebugError) => ExitCode::FAILURE,
            },
        },
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
    /// Enable experimental features (can be used multiple times)
    #[clap(long = "experimental-feature", global = true)]
    experimental_features: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Lint the given files or directories
    Check {
        /// R files to check
        files: Option<Vec<PathBuf>>,
    },
    /// Run the formatter on the given files or directories
    Fmt {
        /// R files to format
        files: Option<Vec<PathBuf>>,
        /// Exit with error if files would be modified without making changes
        #[clap(long, default_value_t = false)]
        check: bool,
        /// Show diff instead of modifying files; exit with error if changes needed
        #[clap(long, default_value_t = false)]
        diff: bool,
    },
    /// Run the language server
    #[clap(alias = "lsp")] // here for backwards compatibility
    Server {
        /// Ignored ... here only to please VS Code
        #[clap(long, default_value_t = true)]
        stdio: bool,
    },
    /// Debugging and development commands
    #[command(subcommand)]
    Debug(Debug),
}

#[derive(Debug, Subcommand)]
enum Debug {
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
