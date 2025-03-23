use {
    clap::{Parser, Subcommand},
    roughly::{
        cli::{self, CheckError, DebugError, FmtError},
        lsp,
    },
    std::{path::PathBuf, process::ExitCode},
};

fn main() -> ExitCode {
    env_logger::init();

    let cli = Cli::parse();
    match cli.command {
        None => {
            lsp::run(cli.experimental);
            ExitCode::SUCCESS
        }
        Some(command) => match command {
            Command::Check { files } => match cli::check(files.as_deref(), cli.experimental) {
                Ok(()) => ExitCode::SUCCESS,
                Err(CheckError) => ExitCode::FAILURE,
            },
            Command::Fmt { files, check, diff } => match cli::fmt(files.as_deref(), check, diff) {
                Ok(()) => ExitCode::SUCCESS,
                Err(FmtError) => ExitCode::FAILURE,
            },
            Command::Lsp { stdio: _stdio } => {
                lsp::run(cli.experimental);
                ExitCode::SUCCESS
            }
            Command::Debug(dev) => match dev {
                Debug::PrintTree { path } => match cli::print_tree(&path) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(DebugError) => ExitCode::FAILURE,
                },
                Debug::Index {
                    paths: files,
                    show_items: all,
                } => match cli::index(files.as_deref(), all) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(DebugError) => ExitCode::FAILURE,
                },
            },
        },
    }
}

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    // #[clap(long, action = clap::ArgAction::HelpLong)]
    // help: Option<bool>,
    /// Ignored ... here only to please VS Code
    #[clap(long, default_value_t = true)]
    stdio: bool,
    /// Enable experimental features
    #[clap(long, global = true, default_value_t = false)]
    experimental: bool,
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
    Lsp {
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
    PrintTree { path: PathBuf },
    /// Index the given files or directories
    Index {
        /// R files to index
        paths: Option<Vec<PathBuf>>,
        /// Show indexed items
        #[clap(long, default_value_t = false)]
        show_items: bool,
    },
}
