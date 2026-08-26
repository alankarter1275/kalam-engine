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
    Meta { book: PathBuf },
    /// Print the table of contents
    Toc { book: PathBuf },
    /// Extract plain text from a spine item (or the whole book)
    Text {
        epub: PathBuf,
        /// Spine index; omit for all spine items
        #[arg(long)]
        spine: Option<usize>,
    },
    /// Dump computed styles per element for a spine item
    Styles {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
    },
    /// Dump the paginated fragment tree for a spine item
    Layout {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
    },
    /// Render a page to PNG
    Render {
        epub: PathBuf,
        #[arg(long, default_value_t = 0)]
        spine: usize,
        #[arg(long, default_value_t = 0)]
        page: usize,
        #[arg(short, long, default_value = "page.png")]
        out: PathBuf,
        /// Color theme: light, sepia, or dark
        #[arg(long, default_value = "light", value_parser = parse_theme)]
        theme: chapbook_core::Theme,
    },
    /// Convert between reading positions and EPUB CFIs
    Cfi {
        epub: PathBuf,
        /// Encode: spine index of the position (with --offset)
        #[arg(long, requires = "offset", conflicts_with = "cfi")]
        spine: Option<usize>,
        /// Encode: locator-text char offset of the position
        #[arg(long)]
        offset: Option<u32>,
        /// Decode: a CFI string, e.g. "epubcfi(/6/8!/4/10/1:10)"
        #[arg(long)]
        cfi: Option<String>,
    },
    /// Browse, search, and download from OPDS catalogs
    Opds {
        #[command(subcommand)]
        command: OpdsCommand,
    },
    /// Manage the local library
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
    /// Import a book into the library
    Import { book: PathBuf },
    /// List library contents, most recently read first
    Ls,
    /// Remove a book from the library (annotations are kept)
    Rm {
        /// Library id, as shown by `lib ls`
        id: i64,
    },
}

fn parse_theme(s: &str) -> Result<chapbook_core::Theme, String> {
    chapbook_core::Theme::from_name(s)
        .ok_or_else(|| format!("unknown theme {s:?} (light|sepia|dark)"))
}

fn main() -> ExitCode {
    // The engine reports through `log`; a dev CLI wants it on stderr where
    // the rest of its output already goes.
    chapbook_core::log_to_stderr();
    let cli = Cli::parse();
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
        Command::Meta { book } => print(commands::meta(&book)),
        Command::Toc { book } => print(commands::toc(&book)),
        Command::Text { epub, spine } => print(commands::text(&epub, spine)),
        Command::Styles { epub, spine } => print(commands::styles(&epub, spine)),
        Command::Layout { epub, spine } => print(commands::layout(&epub, spine)),
        Command::Render {
            epub,
            spine,
            page,
            out,
            theme,
        } => print(commands::render(&epub, spine, page, &out, theme)),
        Command::Cfi {
            epub,
            spine,
            offset,
            cfi,
        } => print(commands::cfi(&epub, spine, offset, cfi.as_deref())),
        Command::Opds { command } => match command {
            OpdsCommand::Ls { url } => print(commands::opds_ls(&url)),
            OpdsCommand::Search { url, query } => print(commands::opds_search(&url, &query)),
            OpdsCommand::Get { url, out } => print(commands::opds_get(&url, &out)),
        },
        Command::Lib { command } => match command {
            LibCommand::Import { book } => print(commands::lib_import(&book)),
            LibCommand::Ls => print(commands::lib_ls()),
            LibCommand::Rm { id } => print(commands::lib_rm(id)),
        },
    }
}
