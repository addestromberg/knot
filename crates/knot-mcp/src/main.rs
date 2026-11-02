use clap::Parser;
use std::{io, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    version,
    about = "Knot MCP server over stdio; read-only unless explicitly enabled"
)]
struct Args {
    #[arg(short = 'C', long, default_value = ".")]
    directory: PathBuf,
    #[arg(long)]
    allow_writes: bool,
}
fn main() -> ExitCode {
    let args = Args::parse();
    match knot_mcp::Server::new(args.directory, args.allow_writes)
        .serve(&mut io::stdin().lock(), &mut io::stdout().lock())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
