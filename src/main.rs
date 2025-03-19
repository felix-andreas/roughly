use {
    clap::{Parser, Subcommand},
    roughly::{
        cli::{self, CheckError, DebugError, FmtError},
        lsp,
    },
    std::{path::PathBuf, process::ExitCode},
};

#[tokio::main]
async fn main() -> ExitCode {
    env_logger::init();

    match Cli::parse().command {
        None => {
            lsp::run().await;
            ExitCode::SUCCESS
        }
        Some(command) => match command {
            Command::Check { files } => match cli::check(files.as_deref()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(CheckError) => ExitCode::FAILURE,
            },
            Command::Fmt { files, check, diff } => match cli::fmt(files.as_deref(), check, diff) {
                Ok(()) => ExitCode::SUCCESS,
                Err(FmtError) => ExitCode::FAILURE,
            },
            Command::Lsp { stdio: _stdio } => {
                lsp::run().await;
                ExitCode::SUCCESS
            }
            Command::Debug(dev) => match dev {
                Debug::PrintTree { path } => match cli::print_tree(&path) {
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
}
