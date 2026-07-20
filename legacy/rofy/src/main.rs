use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about = "Start an R REPL", long_about = None)]
struct Cli {
    /// Use vi mode in the REPL
    #[clap(long, default_value_t = false)]
    vi: bool,
}

fn main() {
    let cli = Cli::parse();
    rofy::run(cli.vi);
}
