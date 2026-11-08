use crate::cache::{self, Kind, RawChange, RawCommit};
use crate::filter;
use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::ObjectId;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Person {
    pub name: String,
    pub email: String,
    pub bot: bool,
}

pub struct Commit {
    pub id: ObjectId,
    pub time: i64,
    pub tz: i32,
    pub author: u32,
    pub summary: String,
    /// Listed in `.git-blame-ignore-revs`.
    pub ignored: bool,
    /// The parent was missing (shallow clone), so the changes are unknown.
    pub partial: bool,
    changes: Range<u32>,
}

#[derive(Clone, Copy)]
pub struct Change {
    /// Index into `History::paths`, by the file's name at the tip.
    pub path: u32,
    pub kind: Kind,
    pub added: u32,
    pub deleted: u32,
    pub binary: bool,
}

pub struct HeadFile {
    pub path: u32,
    pub lines: u32,
    /// Sum of leading indentation, in steps of four columns, over non-blank lines.
    pub complexity: u32,
    pub binary: bool,
}

pub struct History {
    /// The working tree, if the repository has one.
    pub root: Option<std::path::PathBuf>,
    pub name: String,
    pub branch: String,
    pub tip: ObjectId,
    /// Newest first.
    pub commits: Vec<Commit>,
    changes: Vec<Change>,
    pub people: Vec<Person>,
    pub paths: Vec<String>,
    pub head: Vec<HeadFile>,
    /// Patterns from `.gitattributes` files that mark generated or vendored paths: (directory, pattern).
    pub attributes: Vec<(String, String)>,
    /// Commits whose parent could not be read.
    pub partial: usize,
}

impl History {
    pub fn changes(&self, c: &Commit) -> &[Change] {
        &self.changes[c.changes.start as usize..c.changes.end as usize]
    }

    /// Time of the newest commit. Analyses measure age from here, not from the
    /// wall clock, so a quiet repository still has a "recent" past.
    pub fn now(&self) -> i64 {
        self.commits.iter().map(|c| c.time).max().unwrap_or(0)
    }
}

pub struct LoadOptions {
    pub rev: Option<String>,
    pub cache: bool,
    pub progress: bool,
    pub threads: usize,
}

pub fn load(dir: &Path, opt: &LoadOptions) -> Result<History> {
    let mut repo = gix::discover(dir).with_context(|| format!("{} is not inside a git repository", dir.display()))?;
    repo.object_cache_size_if_unset(32 << 20);

    let tip = match &opt.rev {
        Some(r) => {
            repo.rev_parse_single(r.as_str())
                .with_context(|| format!("no revision '{r}'"))?
                .object()?
                .peel_to_commit()
                .with_context(|| format!("'{r}' is not a commit"))?
                .id
        }
        None => repo.head_commit().context("the repository has no commits yet")?.id,
    };
    let branch = match &opt.rev {
        Some(r) => r.clone(),
        None => repo.head_name().ok().flatten().map(|n| n.shorten().to_string()).unwrap_or_else(|| "HEAD".into()),
    };
    let name = repo_name(&repo);

    let progress = Progress::new(opt.progress);
    let mut ids = Vec::new();
    let walk = repo.rev_walk([tip]).sorting(gix::revision::walk::Sorting::ByCommitTime(Default::default())).all()?;
    for info in walk {
        let Ok(info) = info else { break }; // a shallow clone ends here
                                            // Merges are skipped: their changes are already in the commits they merge.
        if info.parent_ids.len() <= 1 {
            ids.push(info.id);
        }
        if ids.len() % 4096 == 0 {
            progress.say(&format!("walking history: {} commits", ids.len()));
        }
    }

    let cache_path = repo.common_dir().join("gmap").join("cache");
    let mut cache = if opt.cache { cache::load(&cache_path) } else { Default::default() };
    let missing: Vec<ObjectId> = ids.iter().filter(|id| !cache.contains_key(*id)).copied().collect();
    if !missing.is_empty() {
        let sync = repo.clone().into_sync();
        let done = AtomicUsize::new(0);
        let results = parallel(missing.len(), opt.threads, &done, &progress, "reading commits", || {
            let mut repo = sync.to_thread_local();
            repo.object_cache_size_if_unset(32 << 20);
            let mut d = Differ::new(repo)?;
            let m = &missing;
            Ok(move |i: usize| d.commit(m[i]).with_context(|| format!("could not read commit {}", m[i])))
        })?;
        for (i, c) in results.into_iter().enumerate() {
            cache.insert(missing[i], c);
        }
        if opt.cache {
            let _ = cache::save(&cache_path, &cache); // a read-only repository still works, just slower
        }
    }

    let tree = repo.find_commit(tip)?.tree()?;
    let mut rec = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut rec)?;
    let blobs: Vec<(String, ObjectId)> = rec
        .records
        .into_iter()
        .filter(|e| e.mode.is_blob())
        .map(|e| (e.filepath.to_str_lossy().into_owned(), e.oid))
        .collect();
    let sync = repo.clone().into_sync();
    let done = AtomicUsize::new(0);
    let measured = parallel(blobs.len(), opt.threads, &done, &progress, "measuring files", || {
        let repo = sync.to_thread_local();
        let blobs = &blobs;
        Ok(move |i: usize| -> Result<(u32, u32, bool, Option<Vec<u8>>)> {
            let blob = repo.find_blob(blobs[i].1)?;
            let (path, data) = (&blobs[i].0, &blob.data);
            let keep = path.ends_with(".gitattributes") || path == ".git-blame-ignore-revs";
            let text = keep.then(|| data.to_vec());
            if is_binary(data) {
                return Ok((0, 0, true, text));
            }
            Ok((count_lines(data), complexity(data), false, text))
        })
    })?;
    progress.clear();

    let mut attributes = Vec::new();
    let mut ignore_revs = Vec::new();
    for ((path, _), (_, _, _, text)) in blobs.iter().zip(&measured) {
        let Some(text) = text else { continue };
        let text = String::from_utf8_lossy(text);
        if path == ".git-blame-ignore-revs" {
            ignore_revs = filter::ignore_revs(&text);
        } else {
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            for p in filter::generated_patterns(&text) {
                attributes.push((dir.to_string(), p));
            }
        }
    }

    let mailmap = repo.open_mailmap();
    let mut b = Builder::default();
    let mut people_by_key: HashMap<String, u32> = HashMap::new();
    let mut commits = Vec::with_capacity(ids.len());
    let mut changes = Vec::new();
    let mut partial = 0;
    for id in &ids {
        let raw = &cache[id];
        let (name, email) = resolve(&mailmap, &raw.name, &raw.email);
        // One person, many emails: work and home addresses, GitHub's noreply.
        // Without a .mailmap, the same full name is the best evidence there is.
        // Names too generic to identify anyone are not merged.
        let by_email = format!("<{}>", email.to_lowercase());
        let by_name = identifying(&name).then(|| name.to_lowercase());
        let found =
            people_by_key.get(&by_email).or_else(|| by_name.as_ref().and_then(|n| people_by_key.get(n))).copied();
        let author = match found {
            Some(p) => p,
            None => {
                b.people.push(Person { bot: filter::is_bot(&name, &email), name: name.clone(), email: email.clone() });
                b.people.len() as u32 - 1
            }
        };
        people_by_key.entry(by_email).or_insert(author);
        if let Some(n) = by_name {
            people_by_key.entry(n).or_insert(author);
        }
        let start = changes.len() as u32;
        for ch in &raw.changes {
            let path = b.canonical(&ch.path);
            if let Some(old) = &ch.old {
                b.alias.insert(old.clone(), path);
            }
            changes.push(Change { path, kind: ch.kind, added: ch.added, deleted: ch.deleted, binary: ch.binary });
        }
        let hex = id.to_string();
        partial += raw.partial as usize;
        commits.push(Commit {
            id: *id,
            time: raw.time,
            tz: raw.tz,
            author,
            summary: raw.summary.clone(),
            ignored: ignore_revs.iter().any(|r| hex.starts_with(r.as_str())),
            partial: raw.partial,
            changes: start..changes.len() as u32,
        });
    }

    let head = blobs
        .iter()
        .zip(measured)
        .map(|((path, _), (lines, complexity, binary, _))| HeadFile { path: b.intern(path), lines, complexity, binary })
        .collect();

    Ok(History {
        root: repo.workdir().map(|p| p.to_path_buf()),
        name,
        branch,
        tip,
        commits,
        changes,
        people: b.people,
        paths: b.paths,
        head,
        attributes,
        partial,
    })
}

#[derive(Default)]
struct Builder {
    paths: Vec<String>,
    ids: HashMap<String, u32>,
    /// Old name -> the file it became, for commits older than the rename.
    alias: HashMap<String, u32>,
    people: Vec<Person>,
}

impl Builder {
    fn intern(&mut self, p: &str) -> u32 {
        if let Some(&i) = self.ids.get(p) {
            return i;
        }
        self.paths.push(p.to_string());
        let i = self.paths.len() as u32 - 1;
        self.ids.insert(p.to_string(), i);
        i
    }

    /// Commits are visited newest first, so by the time an older commit names
    /// a path, any later rename of it has already been recorded.
    fn canonical(&mut self, p: &str) -> u32 {
        match self.alias.get(p) {
            Some(&i) => i,
            None => self.intern(p),
        }
    }
}

fn resolve(mailmap: &gix::mailmap::Snapshot, name: &str, email: &str) -> (String, String) {
    let sig = gix::actor::SignatureRef { name: name.as_bytes().as_bstr(), email: email.as_bytes().as_bstr(), time: "" };
    match mailmap.try_resolve(sig) {
        Some(s) => (s.name.to_str_lossy().into_owned(), s.email.to_str_lossy().into_owned()),
        None => (name.to_string(), email.to_string()),
    }
}

/// A name that could only be one person: two words, and not a placeholder.
fn identifying(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    n.split_whitespace().count() >= 2
        && !["your name", "unknown author", "github action", "github actions", "git user"].contains(&n.as_str())
}

fn repo_name(repo: &gix::Repository) -> String {
    let dir = repo.workdir().unwrap_or_else(|| repo.git_dir());
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let n = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    n.strip_suffix(".git").map(str::to_string).unwrap_or(n)
}

/// Diffs commits against their first parent. One per thread: it keeps gix's
/// diff state and blob buffers between commits.
struct Differ {
    repo: gix::Repository,
    platform: gix::diff::blob::Platform,
    state: gix::diff::tree::State,
    found: Vec<gix::diff::tree_with_rewrites::Change>,
}

impl Differ {
    fn new(repo: gix::Repository) -> Result<Self> {
        let platform = repo.diff_resource_cache_for_tree_diff()?;
        Ok(Self { repo, platform, state: Default::default(), found: Vec::new() })
    }

    fn commit(&mut self, id: ObjectId) -> Result<RawCommit> {
        let commit = self.repo.find_commit(id)?;
        let c = commit.decode()?;
        let author = c.author()?;
        let time = author.time()?;
        let summary = c.message().summary().to_str_lossy().into_owned();
        let name = author.name.to_str_lossy().into_owned();
        let email = author.email.to_str_lossy().into_owned();
        let tree = self.repo.find_tree(c.tree())?;
        let parent = c.parents().next();
        drop(c);

        let old = match parent {
            None => Some(self.repo.empty_tree()),
            Some(p) => self.repo.find_commit(p).ok().and_then(|p| p.tree().ok()),
        };
        let mut out = RawCommit {
            time: time.seconds,
            tz: time.offset,
            name,
            email,
            summary,
            partial: old.is_none(),
            changes: Vec::new(),
        };
        let Some(old) = old else { return Ok(out) };

        self.found.clear();
        let found = &mut self.found;
        gix::diff::tree_with_rewrites(
            gix::objs::TreeRefIter::from_bytes(&old.data, old.id.kind()),
            gix::objs::TreeRefIter::from_bytes(&tree.data, tree.id.kind()),
            &mut self.platform,
            &mut self.state,
            &self.repo.objects,
            |change: gix::diff::tree_with_rewrites::ChangeRef<'_>| -> Result<gix::diff::tree_with_rewrites::Action, gix::Error> {
                found.push(change.into_owned());
                Ok(std::ops::ControlFlow::Continue(()))
            },
            gix::diff::tree_with_rewrites::Options {
                location: Some(gix::diff::tree::recorder::Location::Path),
                rewrites: Some(gix::diff::Rewrites::default()),
            },
        )?;

        use gix::diff::tree_with_rewrites::Change as C;
        for ch in std::mem::take(&mut self.found) {
            // Only blobs are files. Submodules are commits of another repository
            // and symlinks hold a path, so both are skipped.
            let raw = match ch {
                C::Addition { location, entry_mode, id, .. } if entry_mode.is_blob() => {
                    self.whole(Kind::Add, &location, id)?
                }
                C::Deletion { location, entry_mode, id, .. } if entry_mode.is_blob() => {
                    self.whole(Kind::Delete, &location, id)?
                }
                C::Modification { location, previous_entry_mode, previous_id, entry_mode, id } => {
                    match (previous_entry_mode.is_blob(), entry_mode.is_blob()) {
                        (true, true) => self.between(Kind::Modify, &location, None, previous_id, id)?,
                        (false, true) => self.whole(Kind::Add, &location, id)?,
                        (true, false) => self.whole(Kind::Delete, &location, previous_id)?,
                        (false, false) => continue,
                    }
                }
                C::Rewrite { source_location, source_id, entry_mode, id, location, copy, .. }
                    if entry_mode.is_blob() =>
                {
                    if copy {
                        self.whole(Kind::Add, &location, id)?
                    } else {
                        self.between(Kind::Rename, &location, Some(&source_location), source_id, id)?
                    }
                }
                _ => continue,
            };
            out.changes.push(raw);
        }
        Ok(out)
    }

    fn whole(&self, kind: Kind, path: &[u8], id: ObjectId) -> Result<RawChange> {
        let blob = self.repo.find_blob(id)?;
        let binary = is_binary(&blob.data);
        let lines = if binary { 0 } else { count_lines(&blob.data) };
        let (added, deleted) = if kind == Kind::Add { (lines, 0) } else { (0, lines) };
        Ok(RawChange { kind, path: path.to_str_lossy().into_owned(), old: None, added, deleted, binary })
    }

    fn between(&self, kind: Kind, path: &[u8], old: Option<&[u8]>, a: ObjectId, b: ObjectId) -> Result<RawChange> {
        let (added, deleted, binary) = if a == b {
            (0, 0, false)
        } else {
            let a = self.repo.find_blob(a)?;
            let b = self.repo.find_blob(b)?;
            if is_binary(&a.data) || is_binary(&b.data) {
                (0, 0, true)
            } else {
                let (x, y) = line_diff(&a.data, &b.data);
                (x, y, false)
            }
        };
        Ok(RawChange {
            kind,
            path: path.to_str_lossy().into_owned(),
            old: old.map(|o| o.to_str_lossy().into_owned()),
            added,
            deleted,
            binary,
        })
    }
}

/// Lines added and removed, by the histogram algorithm git uses for `--histogram`.
pub fn line_diff(a: &[u8], b: &[u8]) -> (u32, u32) {
    let input = imara_diff::InternedInput::new(a, b);
    let d = imara_diff::Diff::compute(imara_diff::Algorithm::Histogram, &input);
    (d.count_additions(), d.count_removals())
}

/// Git's test: a NUL byte in the first 8000 bytes. Very large files are treated
/// the same way, since nobody reads a 32 MB file line by line.
pub fn is_binary(d: &[u8]) -> bool {
    d.len() > 32 << 20 || d[..d.len().min(8000)].contains(&0)
}

pub fn count_lines(d: &[u8]) -> u32 {
    let n = memchr_count(d);
    n + (!d.is_empty() && d[d.len() - 1] != b'\n') as u32
}

fn memchr_count(d: &[u8]) -> u32 {
    d.iter().filter(|&&b| b == b'\n').count() as u32
}

/// Adam Tornhill's indentation complexity: how deeply nested the code is,
/// summed over lines. A tab counts as four spaces. It needs no parser, works
/// for any language and tracks real complexity metrics closely.
pub fn complexity(d: &[u8]) -> u32 {
    let mut total = 0u32;
    for line in d.split(|&b| b == b'\n') {
        let mut cols = 0u32;
        let mut blank = true;
        for &b in line {
            match b {
                b' ' => cols += 1,
                b'\t' => cols += 4,
                b'\r' => {}
                _ => {
                    blank = false;
                    break;
                }
            }
        }
        if !blank {
            total += cols / 4;
        }
    }
    total
}

/// Run `make()` once per thread to build a worker, then hand out indices
/// `0..n` in small batches until none are left. Results come back in order.
fn parallel<T, F, M>(
    n: usize,
    threads: usize,
    done: &AtomicUsize,
    progress: &Progress,
    what: &str,
    make: M,
) -> Result<Vec<T>>
where
    T: Send,
    F: FnMut(usize) -> Result<T>,
    M: Fn() -> Result<F> + Sync,
{
    const BATCH: usize = 8;
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let threads = threads.clamp(1, n.max(1).div_ceil(BATCH));
    let parts: Vec<Result<Vec<(usize, T)>>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> Result<Vec<(usize, T)>> {
                    let mut f = make()?;
                    let mut out = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        let i = next.fetch_add(BATCH, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        for j in i..(i + BATCH).min(n) {
                            match f(j) {
                                Ok(v) => out.push((j, v)),
                                Err(e) => {
                                    stop.store(true, Ordering::Relaxed);
                                    return Err(e);
                                }
                            }
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        while handles.iter().any(|h| !h.is_finished()) {
            progress.say(&format!("{what}: {}/{n}", done.load(Ordering::Relaxed).min(n)));
            std::thread::sleep(Duration::from_millis(50));
        }
        handles.into_iter().map(|h| h.join().expect("worker panicked")).collect()
    });
    let mut slots: Vec<Option<T>> = (0..n).map(|_| None).collect();
    for part in parts {
        for (i, v) in part? {
            slots[i] = Some(v);
        }
    }
    Ok(slots.into_iter().map(|v| v.expect("every index is filled")).collect())
}

/// A one-line status on stderr, only for a terminal and only after the work
/// has taken long enough to be worth watching.
struct Progress {
    on: bool,
    start: Instant,
    shown: AtomicBool,
}

impl Progress {
    fn new(on: bool) -> Self {
        Self { on: on && std::io::stderr().is_terminal(), start: Instant::now(), shown: AtomicBool::new(false) }
    }
    fn say(&self, msg: &str) {
        if self.on && self.start.elapsed() > Duration::from_millis(300) {
            eprint!("\r\x1b[2K{msg}");
            self.shown.store(true, Ordering::Relaxed);
        }
    }
    fn clear(&self) {
        if self.shown.load(Ordering::Relaxed) {
            eprint!("\r\x1b[2K");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b"a\nb"), 2);
    }

    #[test]
    fn indentation() {
        let code = b"fn f() {\n    if x {\n\t\ty();\n    }\n\n        \n}\n";
        // 0 + 1 + 2 + 1, blank and whitespace-only lines skipped
        assert_eq!(complexity(code), 4);
    }

    #[test]
    fn diffs() {
        assert_eq!(line_diff(b"a\nb\nc\n", b"a\nB\nc\nd\n"), (2, 1));
        assert_eq!(line_diff(b"", b"x\ny\n"), (2, 0));
    }

    #[test]
    fn binary() {
        assert!(is_binary(b"PNG\0\x01"));
        assert!(!is_binary(b"plain text"));
    }
}
