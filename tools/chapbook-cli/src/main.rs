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
    /// Manage the local library
    Lib {
        #[command(subcommand)]
        command: LibCommand,
    },
}

#[derive(Subcommand)]
enum LibCommand {
    /// Import a book into the library
    Import { book: PathBuf },
    /// List library contents, most recently read first
    Ls {
        /// Search title, authors and series. Whole words, prefix-matched,
        /// and folded for case and accents: "bronte" finds Brontë
        #[arg(long, short)]
        search: Option<String>,
        /// Only books in this collection, by name
        #[arg(long)]
        collection: Option<String>,
        /// Only books in this series
        #[arg(long)]
        series: Option<String>,
        /// unread, reading or finished
        #[arg(long, value_parser = parse_state)]
        state: Option<chapbook_library::ReadingState>,
        /// added, read, title, author or series
        #[arg(long, default_value = "read", value_parser = parse_sort)]
        sort: chapbook_library::Sort,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Everything the library knows about one book
    Show {
        /// Library id, as shown by `lib ls`
        id: i64,
    },
    /// Remove a book from the library (annotations are kept)
    Rm {
        /// Library id, as shown by `lib ls`
        id: i64,
    },
    /// Mark a book as finished
    Finish {
        id: i64,
        /// Take the mark back
        #[arg(long)]
        undo: bool,
    },
    /// List the series on the shelf, with how many books are in each
    Series,
    /// Group books into named collections
    Collection {
        #[command(subcommand)]
        command: CollectionCommand,
    },
}

#[derive(Subcommand)]
enum CollectionCommand {
    /// List collections and their sizes
    Ls,
    /// Create an empty collection
    New { name: String },
    /// Rename a collection
    Rename { name: String, new_name: String },
    /// Delete a collection. The books stay; only the grouping goes
    Rm { name: String },
    /// Put a book in a collection, creating the collection if needed
    Add { name: String, id: i64 },
    /// Take a book out of a collection
    Remove { name: String, id: i64 },
}

fn parse_state(s: &str) -> std::result::Result<chapbook_library::ReadingState, String> {
    use chapbook_library::ReadingState;
    match s {
        "unread" => Ok(ReadingState::Unread),
        "reading" => Ok(ReadingState::Reading),
        "finished" => Ok(ReadingState::Finished),
        _ => Err(format!("unknown state {s:?} (unread|reading|finished)")),
    }
}

fn parse_sort(s: &str) -> std::result::Result<chapbook_library::Sort, String> {
    use chapbook_library::Sort;
    match s {
        "added" => Ok(Sort::Added),
        "read" => Ok(Sort::Read),
        "title" => Ok(Sort::Title),
        "author" => Ok(Sort::Author),
        "series" => Ok(Sort::Series),
        _ => Err(format!(
            "unknown sort {s:?} (added|read|title|author|series)"
        )),
    }
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
        Command::Lib { command } => match command {
            LibCommand::Import { book } => print(commands::lib_import(&book)),
            LibCommand::Ls {
                search,
                collection,
                series,
                state,
                sort,
                limit,
            } => print(commands::lib_ls(
                search.as_deref(),
                collection.as_deref(),
                series.as_deref(),
                state,
                sort,
                limit,
            )),
            LibCommand::Show { id } => print(commands::lib_show(id)),
            LibCommand::Rm { id } => print(commands::lib_rm(id)),
            LibCommand::Finish { id, undo } => print(commands::lib_finish(id, !undo)),
            LibCommand::Series => print(commands::lib_series()),
            LibCommand::Collection { command } => match command {
                CollectionCommand::Ls => print(commands::lib_collections()),
                CollectionCommand::New { name } => print(commands::lib_collection_new(&name)),
                CollectionCommand::Rename { name, new_name } => {
                    print(commands::lib_collection_rename(&name, &new_name))
                }
                CollectionCommand::Rm { name } => print(commands::lib_collection_rm(&name)),
                CollectionCommand::Add { name, id } => {
                    print(commands::lib_collection_add(&name, id))
                }
                CollectionCommand::Remove { name, id } => {
                    print(commands::lib_collection_remove(&name, id))
                }
            },
        },
    }
}
