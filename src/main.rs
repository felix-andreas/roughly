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
                Err(()) => ExitCode::FAILURE,
            },
            Command::Fmt { files, check, diff } => {
                match format::run(files.as_deref(), check, diff) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(()) => ExitCode::FAILURE,
                }
            }
            Command::Lsp { stdio: _stdio } => {
                lsp::run().await;
                ExitCode::SUCCESS
            }
            Command::Dev(dev) => match dev {
                Dev::Sexp { path } => match dev::sexp(&path) {
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
    },
    /// Run the language server
    Lsp {
        /// Ignored ... here only to please VS Code
        #[clap(long, default_value_t = true)]
        stdio: bool,
    },
    /// Collection of useful commands
    #[command(subcommand)]
    Dev(Dev),
}

#[derive(Debug, Subcommand)]
enum Dev {
    /// Print the Sexp for the given file
    Sexp { path: PathBuf },
}
