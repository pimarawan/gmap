mod analysis;
mod app;
mod cache;
mod filter;
mod history;
mod paint;
mod report;
mod time;
mod treemap;
mod ui;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// Find the risky parts of a git repository.
///
/// With no command, opens the map: every file as a rectangle sized by its
/// lines and colored by how often it changes.
#[derive(Parser)]
#[command(version, max_term_width = 100)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    opts: Opts,
}

#[derive(Args, Clone)]
struct Opts {
    /// Repository to read [default: the current directory]
    #[arg(short = 'C', long = "repo", global = true, value_name = "DIR")]
    repo: Option<PathBuf>,
    /// Read history up to this revision instead of HEAD
    #[arg(long, global = true, value_name = "REV")]
    rev: Option<String>,
    /// Only commits after this: 2024-01-01, 2024-06, 6m, 1y
    #[arg(short, long, global = true, value_name = "WHEN")]
    since: Option<String>,
    /// Only commits before this
    #[arg(short, long, global = true, value_name = "WHEN")]
    until: Option<String>,
    /// Only commits whose author name or email contains this
    #[arg(short, long, global = true, value_name = "TEXT")]
    author: Option<String>,
    /// Leave out paths matching this gitignore-style pattern (repeatable)
    #[arg(short = 'x', long, global = true, value_name = "GLOB")]
    exclude: Vec<String>,
    /// Look at every file: docs, configuration, lockfiles, vendored and generated code too
    #[arg(long, global = true)]
    all: bool,
    /// Count commits by bots (dependabot, renovate, ...)
    #[arg(long, global = true)]
    bots: bool,
    /// A change this old counts half as much as one made today
    #[arg(long, global = true, value_name = "TIME", default_value = "1y")]
    half_life: String,
    /// Someone with no commit for this long has left
    #[arg(long, global = true, value_name = "TIME", default_value = "1y")]
    inactive: String,
    /// Do not read or write the cache in .git/gmap
    #[arg(long, global = true)]
    no_cache: bool,
    /// Threads for reading history [default: one per CPU]
    #[arg(short = 'j', long, global = true, value_name = "N")]
    threads: Option<usize>,
}

#[derive(Subcommand)]
enum Command {
    /// Files that are complicated and keep changing
    Hotspots(List),
    /// Files that change in the same commits
    Coupling {
        #[command(flatten)]
        list: List,
        /// Only pairs in different directories, the surprising ones
        #[arg(long)]
        across: bool,
        /// Ignore commits that touch more files than this
        #[arg(long, value_name = "N", default_value_t = 30)]
        max_files: usize,
        /// Pairs need at least this many shared commits
        #[arg(long, value_name = "N", default_value_t = 3)]
        min_shared: u32,
    },
    /// Who knows the code, and how many people the project can lose
    Owners(List),
    /// When commits happen
    #[command(alias = "heat")]
    Activity(Json),
    /// Lines added and deleted per file or directory
    Churn {
        #[command(flatten)]
        list: List,
        /// Add up by the first N path components
        #[arg(long, value_name = "N")]
        depth: Option<usize>,
    },
    /// Every change as one JSON object per line
    Export {
        /// Only files under this path
        path: Option<String>,
    },
}

#[derive(Args)]
struct List {
    /// Only files under this path
    path: Option<String>,
    /// How many to show
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    /// Print JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct Json {
    /// Only files under this path
    path: Option<String>,
    /// Print JSON
    #[arg(long)]
    json: bool,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::BrokenPipe {
                return;
            }
        }
        eprintln!("gmap: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let o = &cli.opts;
    let dir = o.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    if cli.command.is_none() && !std::io::stdout().is_terminal() {
        anyhow::bail!("the map needs a terminal; try `gmap hotspots` for text output");
    }
    let h = history::load(
        &dir,
        &history::LoadOptions {
            rev: o.rev.clone(),
            cache: !o.no_cache,
            progress: true,
            threads: o.threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get())),
        },
    )?;
    if h.commits.is_empty() {
        anyhow::bail!("no commits to read");
    }
    let now = h.now();
    let path = match &cli.command {
        Some(Command::Hotspots(l) | Command::Owners(l)) => l.path.clone(),
        Some(Command::Coupling { list, .. } | Command::Churn { list, .. }) => list.path.clone(),
        Some(Command::Activity(j)) => j.path.clone(),
        Some(Command::Export { path }) => path.clone(),
        None => None,
    };
    let settings = analysis::Settings {
        since: o.since.as_deref().map(|s| time::parse_when(s, now)).transpose()?,
        until: o.until.as_deref().map(|s| time::parse_when(s, now)).transpose()?,
        author: o.author.clone(),
        prefix: path.map(|p| p.replace('\\', "/").trim_start_matches("./").to_string()),
        bots: o.bots,
        code_only: !o.all,
        half_life: time::parse_duration(&o.half_life)?.max(time::DAY),
        inactive: time::parse_duration(&o.inactive)?,
        max_changeset: match &cli.command {
            Some(Command::Coupling { max_files, .. }) => *max_files,
            _ => 30,
        },
    };
    let filter = filter::PathFilter::new(!o.all, &o.exclude, &h.attributes)?;

    let Some(command) = cli.command else {
        let everything = filter::PathFilter::new(false, &o.exclude, &[])?;
        return app::run(h, [filter, everything], settings);
    };

    let a = analysis::analyze(&h, &filter, &settings);
    let stdout = std::io::stdout();
    let tty = stdout.is_terminal();
    let width = if tty { crossterm::terminal::size().map_or(100, |s| s.0 as usize) } else { usize::MAX };
    let color = tty && std::env::var_os("NO_COLOR").is_none();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let ctx = |limit| report::Ctx { h: &h, a: &a, filter: &filter, s: &settings, color, width, limit };
    match command {
        Command::Hotspots(l) => report::hotspots(&ctx(l.limit), l.json, &mut out)?,
        Command::Coupling { list, across, max_files, min_shared } => {
            let cp = analysis::coupling(&h, &a, max_files, min_shared.max(2));
            report::coupling(&ctx(list.limit), &cp, across, list.json, &mut out)?
        }
        Command::Owners(l) => report::owners(&ctx(l.limit), l.json, &mut out)?,
        Command::Activity(j) => report::activity(&ctx(usize::MAX), j.json, &mut out)?,
        Command::Churn { list, depth } => report::churn(&ctx(list.limit), depth, list.json, &mut out)?,
        Command::Export { .. } => report::export(&ctx(usize::MAX), &mut out)?,
    }
    out.flush()?;
    Ok(())
}
