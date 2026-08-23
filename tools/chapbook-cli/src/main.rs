//! Development CLI: each subcommand ships with a milestone and doubles as its
//! test harness (snapshot inputs come from running these against fixtures).

use chapbook_cli::commands;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "chapbook", version, about = "chapbook ereader pipeline tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print book metadata (title, authors, language, identifiers)
    Meta { epub: PathBuf },
    /// Print the table of contents
    Toc { epub: PathBuf },
    /// Extract plain text from a spine item (or the whole book)
    Text {
        epub: PathBuf,
        /// Spine index; omit for all spine items
        #[arg(long)]
        spine: Option<usize>,
    },
    /// Dump computed styles per element for a spine item (M2)
    Styles {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
    },
    /// Dump the paginated fragment tree for a spine item (M3)
    Layout {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
    },
    /// Render a page to PNG (M4)
    Render {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
        #[arg(long, default_value_t = 0)]
        page: usize,
        #[arg(short, long, default_value = "page.png")]
        out: PathBuf,
    },
    /// Browse, search, and download from OPDS catalogs (M6)
    Opds {
        #[command(subcommand)]
        command: OpdsCommand,
    },
    /// Manage the local library (M7)
    Lib {
        #[command(subcommand)]
        command: LibCommand,
    },
}

#[derive(Subcommand)]
enum OpdsCommand {
    /// List entries of a catalog feed
    Ls { url: String },
    /// Search a catalog
    Search { url: String, query: String },
    /// Download an open-access acquisition
    Get { url: String, out: PathBuf },
}

#[derive(Subcommand)]
enum LibCommand {
    /// Import an EPUB into the library
    Import { epub: PathBuf },
    /// List library contents
    Ls,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let unimplemented = |what: &str, milestone: &str| -> ExitCode {
        eprintln!("chapbook {what}: not yet implemented (lands in milestone {milestone})");
        ExitCode::FAILURE
    };
    let print = |result: chapbook_core::Result<String>| -> ExitCode {
        match result {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("chapbook: {e}");
                ExitCode::FAILURE
            }
        }
    };
    match cli.command {
        Command::Meta { epub } => print(commands::meta(&epub)),
        Command::Toc { epub } => print(commands::toc(&epub)),
        Command::Text { epub, spine } => print(commands::text(&epub, spine)),
        Command::Styles { epub, spine } => print(commands::styles(&epub, spine)),
        Command::Layout { .. } => unimplemented("layout", "M3"),
        Command::Render { .. } => unimplemented("render", "M4"),
        Command::Opds { .. } => unimplemented("opds", "M6"),
        Command::Lib { .. } => unimplemented("lib", "M7"),
    }
}
