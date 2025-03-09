use {
    clap::{Parser, Subcommand},
    roughly::{cli, dev, diagnostics, format, lsp},
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
            Command::Check { files } => match diagnostics::run(files.as_deref()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            },
            Command::Fmt {
                files,
                check,
                diff,
                stop_on_unhandled_comment,
            } => match format::run(files.as_deref(), check, diff, stop_on_unhandled_comment) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            },
            Command::Lsp { stdio: _stdio } => {
                lsp::run().await;
                ExitCode::SUCCESS
            }
            Command::Debug(dev) => match dev {
                Debug::PrintTree { path } => match dev::tree(&path) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        cli::error(&err.to_string());
                        ExitCode::FAILURE
                    }
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
        /// Avoid writing any formatted files back; instead, exit with a non-zero status code if any files would have been modified, and zero otherwise
        #[clap(long, default_value_t = false)]
        check: bool,
        /// Avoid writing any formatted files back; instead, exit with a non-zero status code and the difference between the current file and how the formatted file would look like
        #[clap(long, default_value_t = false)]
        diff: bool,
        /// Stop if a comment can't be properly placed (exits with error if a comment would be moved to different location)
        #[clap(long, default_value_t = false)]
        stop_on_unhandled_comment: bool,
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
