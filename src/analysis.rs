use crate::filter::PathFilter;
use crate::history::History;
use crate::time::DAY;
use std::collections::HashMap;

pub struct Settings {
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Case-insensitive substring of an author's name or email.
    pub author: Option<String>,
    /// Only files under this path.
    pub prefix: Option<String>,
    pub bots: bool,
    /// Only source code: no docs, configuration or data.
    pub code_only: bool,
    /// A change this old counts half as much as one made today.
    pub half_life: i64,
    /// Someone with no commit for this long is gone.
    pub inactive: i64,
    /// Commits touching more files than this say nothing about coupling.
    pub max_changeset: usize,
}

pub struct FileStat {
    pub path: u32,
    pub lines: u32,
    pub complexity: u32,
    pub revs: u32,
    /// Changes, each weighted by 2^(-age / half-life).
    pub weight: f64,
    pub added: u64,
    pub deleted: u64,
    pub first: i64,
    pub last: i64,
    pub creator: Option<u32>,
    /// (person, commits), most commits first.
    pub authors: Vec<(u32, u32)>,
    /// People who know the file, by degree of authorship, strongest first.
    pub owners: Vec<u32>,
    /// Indices into `History::commits`, newest first.
    pub commits: Vec<u32>,
    pub score: f64,
}

pub struct PersonStat {
    pub person: u32,
    pub commits: u32,
    pub added: u64,
    pub deleted: u64,
    pub first: i64,
    /// Last commit anywhere in the history, not just in the window.
    pub last: i64,
    /// Files this person is an owner of.
    pub owns: u32,
    pub active: bool,
}

pub struct Analysis {
    pub now: i64,
    /// Commits in the window, newest first.
    pub commits: Vec<u32>,
    pub files: Vec<FileStat>,
    /// Path id -> index into `files`.
    index: Vec<u32>,
    /// Most commits first.
    pub people: Vec<PersonStat>,
    person_index: Vec<u32>,
    /// Files at the tip left out by the path filter.
    pub excluded: usize,
}

const NONE: u32 = u32::MAX;

impl Analysis {
    pub fn file_index(&self, path: u32) -> Option<usize> {
        self.index.get(path as usize).filter(|&&i| i != NONE).map(|&i| i as usize)
    }

    pub fn person(&self, person: u32) -> Option<&PersonStat> {
        match self.person_index.get(person as usize) {
            Some(&i) if i != NONE => Some(&self.people[i as usize]),
            _ => None,
        }
    }

    /// Files by hotspot score, highest first. Files that never changed are left out.
    pub fn hotspots(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.files.len()).filter(|&i| self.files[i].revs > 0).collect();
        v.sort_by(|&a, &b| {
            self.files[b].score.total_cmp(&self.files[a].score).then(self.files[b].revs.cmp(&self.files[a].revs))
        });
        v
    }
}

pub fn analyze(h: &History, filter: &PathFilter, s: &Settings) -> Analysis {
    let now = h.now();
    let mut index = vec![NONE; h.paths.len()];
    let mut files = Vec::new();
    let mut excluded = 0;
    for f in &h.head {
        let path = &h.paths[f.path as usize];
        if let Some(p) = &s.prefix {
            if !under(path, p) {
                continue;
            }
        }
        if filter.excludes(path) || f.binary || (s.code_only && !crate::filter::is_code(path)) {
            excluded += 1;
            continue;
        }
        index[f.path as usize] = files.len() as u32;
        files.push(FileStat {
            path: f.path,
            lines: f.lines,
            complexity: f.complexity,
            revs: 0,
            weight: 0.0,
            added: 0,
            deleted: 0,
            first: i64::MAX,
            last: i64::MIN,
            creator: None,
            authors: Vec::new(),
            owners: Vec::new(),
            commits: Vec::new(),
            score: 0.0,
        });
    }

    let is_human = |p: u32| s.bots || !h.people[p as usize].bot;

    // Who created each file, over the whole history: the oldest addition wins.
    let mut last_seen = vec![i64::MIN; h.people.len()];
    for c in h.commits.iter().rev() {
        last_seen[c.author as usize] = last_seen[c.author as usize].max(c.time);
        if !is_human(c.author) {
            continue;
        }
        for ch in h.changes(c) {
            let i = index[ch.path as usize];
            if i != NONE && ch.kind == crate::cache::Kind::Add && files[i as usize].creator.is_none() {
                files[i as usize].creator = Some(c.author);
            }
        }
    }

    let needle = s.author.as_ref().map(|a| a.to_lowercase());
    let mut commits = Vec::new();
    let mut counts: Vec<HashMap<u32, u32>> = (0..files.len()).map(|_| HashMap::new()).collect();
    let mut people: HashMap<u32, PersonStat> = HashMap::new();
    let ln2 = std::f64::consts::LN_2;
    for (ci, c) in h.commits.iter().enumerate() {
        if c.ignored || c.partial || !is_human(c.author) {
            continue;
        }
        if s.since.is_some_and(|t| c.time < t) || s.until.is_some_and(|t| c.time >= t) {
            continue;
        }
        if let Some(n) = &needle {
            let p = &h.people[c.author as usize];
            if !p.name.to_lowercase().contains(n) && !p.email.to_lowercase().contains(n) {
                continue;
            }
        }
        commits.push(ci as u32);
        let w = (-((now - c.time).max(0) as f64) / s.half_life as f64 * ln2).exp();
        let ps = people.entry(c.author).or_insert_with(|| PersonStat {
            person: c.author,
            commits: 0,
            added: 0,
            deleted: 0,
            first: c.time,
            last: last_seen[c.author as usize],
            owns: 0,
            active: false,
        });
        ps.commits += 1;
        ps.first = ps.first.min(c.time);
        for ch in h.changes(c) {
            let i = index[ch.path as usize];
            if i == NONE {
                continue;
            }
            let f = &mut files[i as usize];
            if f.commits.last() == Some(&(ci as u32)) {
                continue;
            }
            f.revs += 1;
            f.weight += w;
            f.added += ch.added as u64;
            f.deleted += ch.deleted as u64;
            f.first = f.first.min(c.time);
            f.last = f.last.max(c.time);
            f.commits.push(ci as u32);
            *counts[i as usize].entry(c.author).or_default() += 1;
            ps.added += ch.added as u64;
            ps.deleted += ch.deleted as u64;
        }
    }

    for (f, counts) in files.iter_mut().zip(counts) {
        f.score = f.weight * (1.0 + f.complexity as f64).ln();
        f.authors = counts.into_iter().collect();
        f.authors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        f.owners = owners(f);
        for &o in &f.owners {
            if let Some(p) = people.get_mut(&o) {
                p.owns += 1;
            }
        }
    }

    let mut people: Vec<PersonStat> = people.into_values().collect();
    for p in &mut people {
        p.active = now - p.last <= s.inactive;
    }
    people.sort_by(|a, b| b.commits.cmp(&a.commits).then(a.person.cmp(&b.person)));
    let mut person_index = vec![NONE; h.people.len()];
    for (i, p) in people.iter().enumerate() {
        person_index[p.person as usize] = i as u32;
    }

    Analysis { now, commits, files, index, people, person_index, excluded }
}

pub fn under(path: &str, prefix: &str) -> bool {
    let p = prefix.trim_matches('/');
    p.is_empty() || path == p || (path.starts_with(p) && path.as_bytes().get(p.len()) == Some(&b'/'))
}

/// Degree of authorship, Fritz et al. (2014), with the weights Avelino et al.
/// fitted on 120 GitHub projects:
///
///   DOA = 3.293 + 1.098 FA + 0.164 DL - 0.321 ln(1 + AC)
///
/// FA is 1 for whoever created the file, DL is your commits to it, AC is
/// everyone else's. You know a file if your DOA is at least 3.293 and at least
/// 75% of the highest DOA on that file.
pub fn doa(created: bool, mine: u32, others: u32) -> f64 {
    3.293 + 1.098 * created as u8 as f64 + 0.164 * mine as f64 - 0.321 * (1.0 + others as f64).ln()
}

fn owners(f: &FileStat) -> Vec<u32> {
    let scored: Vec<(u32, f64)> =
        f.authors.iter().map(|&(p, n)| (p, doa(f.creator == Some(p), n, f.revs - n))).collect();
    let best = scored.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    let mut v: Vec<(u32, f64)> = scored.into_iter().filter(|&(_, d)| d >= 3.293 && d >= 0.75 * best).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.into_iter().map(|(p, _)| p).collect()
}

pub struct Truck {
    /// How many people can leave before more than half the files have no owner.
    pub factor: usize,
    /// Those people, in the order they were removed.
    pub key: Vec<u32>,
    /// Share of files still owned after each removal, starting before any.
    pub coverage: Vec<f64>,
    pub files: usize,
}

/// Avelino et al.: rank people by how many files they own, then remove them
/// one at a time until more than half the files have nobody left who knows them.
pub fn truck_factor(a: &Analysis, files: &[usize]) -> Truck {
    let total = files.len();
    let mut owns: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut left: Vec<u32> = Vec::with_capacity(total);
    for (k, &i) in files.iter().enumerate() {
        left.push(a.files[i].owners.len() as u32);
        for &o in &a.files[i].owners {
            owns.entry(o).or_default().push(k);
        }
    }
    let mut ranked: Vec<(u32, Vec<usize>)> = owns.into_iter().collect();
    ranked.sort_by(|x, y| {
        let cx = a.person(x.0).map_or(0, |p| p.commits);
        let cy = a.person(y.0).map_or(0, |p| p.commits);
        y.1.len().cmp(&x.1.len()).then(cy.cmp(&cx)).then(x.0.cmp(&y.0))
    });

    let mut covered = left.iter().filter(|&&n| n > 0).count();
    let share = |c: usize| if total == 0 { 0.0 } else { c as f64 / total as f64 };
    let mut coverage = vec![share(covered)];
    let mut key = Vec::new();
    for (p, owned) in ranked {
        if total == 0 || covered * 2 < total {
            break;
        }
        for k in owned {
            left[k] -= 1;
            if left[k] == 0 {
                covered -= 1;
            }
        }
        key.push(p);
        coverage.push(share(covered));
    }
    Truck { factor: key.len(), key, coverage, files: total }
}

/// Files whose owners have all stopped committing.
pub fn orphans(a: &Analysis, files: &[usize]) -> Vec<usize> {
    files
        .iter()
        .copied()
        .filter(|&i| {
            let f = &a.files[i];
            !f.owners.is_empty() && f.owners.iter().all(|&o| a.person(o).is_some_and(|p| !p.active))
        })
        .collect()
}

pub struct Pair {
    pub a: usize,
    pub b: usize,
    pub shared: u32,
    /// Commits that changed each file, of those the test counts: no
    /// creations, no commits over the size limit.
    pub revs_a: u32,
    pub revs_b: u32,
    /// Shared commits over the average of the two files' commits (code-maat's degree).
    pub degree: f64,
    /// Chance of at least `shared` co-changes if the two files changed independently.
    pub p: f64,
}

pub struct Coupling {
    pub pairs: Vec<Pair>,
    /// Pairs that were tested; the significance threshold is 0.05 / tested.
    pub tested: usize,
    /// Commits small enough to count.
    pub commits: usize,
}

pub fn coupling(h: &History, a: &Analysis, max_changeset: usize, min_shared: u32) -> Coupling {
    let mut revs = vec![0u32; a.files.len()];
    let mut shared: HashMap<(u32, u32), u32> = HashMap::new();
    let mut n = 0usize;
    let mut set: Vec<u32> = Vec::new();
    for &ci in &a.commits {
        set.clear();
        for ch in h.changes(&h.commits[ci as usize]) {
            // Files created in the same commit, as when a project is first
            // imported, were not changed together; they just appeared together.
            if ch.kind == crate::cache::Kind::Add {
                continue;
            }
            if let Some(i) = a.file_index(ch.path) {
                set.push(i as u32);
            }
        }
        set.sort_unstable();
        set.dedup();
        if set.is_empty() || set.len() > max_changeset {
            continue;
        }
        n += 1;
        for (k, &x) in set.iter().enumerate() {
            revs[x as usize] += 1;
            for &y in &set[k + 1..] {
                *shared.entry((x, y)).or_default() += 1;
            }
        }
    }

    let lf = LnFactorial::new(n);
    let candidates: Vec<((u32, u32), u32)> = shared.into_iter().filter(|&(_, s)| s >= min_shared).collect();
    let tested = candidates.len();
    let alpha = 0.05 / tested.max(1) as f64;
    let mut pairs: Vec<Pair> = candidates
        .into_iter()
        .filter_map(|((x, y), s)| {
            let (rx, ry) = (revs[x as usize], revs[y as usize]);
            let degree = s as f64 / ((rx + ry) as f64 / 2.0);
            let p = hypergeometric_sf(&lf, n as u64, rx as u64, ry as u64, s as u64);
            (p <= alpha && degree >= 0.25).then_some(Pair {
                a: x as usize,
                b: y as usize,
                shared: s,
                revs_a: rx,
                revs_b: ry,
                degree,
                p,
            })
        })
        .collect();
    // Rank by degree shrunk toward zero by two phantom commits, so 3 out of 3
    // does not outrank 40 out of 45: small samples earn less trust.
    let rank = |p: &Pair| p.shared as f64 / ((p.revs_a + p.revs_b) as f64 / 2.0 + 2.0);
    pairs.sort_by(|x, y| {
        rank(y).total_cmp(&rank(x)).then(y.shared.cmp(&x.shared)).then(x.a.cmp(&y.a)).then(x.b.cmp(&y.b))
    });
    Coupling { pairs, tested, commits: n }
}

/// ln(k!) by table for small k and Stirling's series beyond it.
pub struct LnFactorial {
    table: Vec<f64>,
}

impl LnFactorial {
    pub fn new(n: usize) -> Self {
        let m = (n + 1).min(1 << 16);
        let mut table = Vec::with_capacity(m);
        let mut acc = 0.0;
        table.push(0.0);
        for k in 1..m {
            acc += (k as f64).ln();
            table.push(acc);
        }
        Self { table }
    }

    pub fn get(&self, k: u64) -> f64 {
        if let Some(&v) = self.table.get(k as usize) {
            return v;
        }
        let x = k as f64;
        x * x.ln() - x + 0.5 * (std::f64::consts::TAU * x).ln() + 1.0 / (12.0 * x) - 1.0 / (360.0 * x * x * x)
    }

    fn choose(&self, n: u64, k: u64) -> f64 {
        self.get(n) - self.get(k) - self.get(n - k)
    }
}

/// P(X >= k) for X ~ Hypergeometric(N commits, K changed file a, n changed file b):
/// the chance that `a` and `b`, changing independently, share at least k commits.
pub fn hypergeometric_sf(lf: &LnFactorial, total: u64, ka: u64, kb: u64, k: u64) -> f64 {
    let hi = ka.min(kb);
    if k > hi {
        return 0.0;
    }
    let lo = (ka + kb).saturating_sub(total);
    if k <= lo {
        return 1.0;
    }
    let denom = lf.choose(total, kb);
    let term = |i: u64| lf.choose(ka, i) + lf.choose(total - ka, kb - i) - denom;
    // Sum in log space from the largest term so nothing underflows.
    let terms: Vec<f64> = (k..=hi).map(term).collect();
    let m = terms.iter().copied().fold(f64::MIN, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - m).exp()).sum();
    (m + sum.ln()).exp().min(1.0)
}

pub struct Activity {
    /// Commits per day in the authors' local time: (day number, count), ascending.
    pub days: Vec<(i64, u32)>,
    /// Commits by weekday (Monday first) and hour, in the authors' local time.
    pub punch: [[u32; 24]; 7],
    pub first: i64,
    pub last: i64,
}

pub fn activity(h: &History, commits: &[u32]) -> Activity {
    let mut days: HashMap<i64, u32> = HashMap::new();
    let mut punch = [[0u32; 24]; 7];
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for &ci in commits {
        let c = &h.commits[ci as usize];
        let local = c.time + c.tz as i64;
        let d = local.div_euclid(DAY);
        *days.entry(d).or_default() += 1;
        punch[crate::time::weekday(d)][(local.rem_euclid(DAY) / 3600) as usize] += 1;
        first = first.min(c.time);
        last = last.max(c.time);
    }
    let mut days: Vec<(i64, u32)> = days.into_iter().collect();
    days.sort_unstable();
    Activity { days, punch, first, last }
}

/// Changes per bucket over a span, for sparklines: `buckets` equal slices of
/// `[from, to]`, counting the given commits.
pub fn histogram(h: &History, commits: &[u32], from: i64, to: i64, buckets: usize) -> Vec<u32> {
    let mut v = vec![0u32; buckets.max(1)];
    let span = (to - from).max(1) as f64;
    for &ci in commits {
        let t = h.commits[ci as usize].time;
        if t < from || t > to {
            continue;
        }
        let k = (((t - from) as f64 / span) * v.len() as f64) as usize;
        let len = v.len();
        v[k.min(len - 1)] += 1;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binom(n: u64, k: u64) -> f64 {
        (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
    }

    #[test]
    fn hypergeometric_matches_direct_sum() {
        let lf = LnFactorial::new(60);
        for &(n, ka, kb, k) in
            &[(60u64, 10u64, 12u64, 5u64), (60, 30, 30, 15), (60, 5, 5, 5), (60, 20, 50, 15), (60, 7, 9, 0)]
        {
            let direct: f64 = (k..=ka.min(kb))
                .filter(|&i| kb - i <= n - ka)
                .map(|i| binom(ka, i) * binom(n - ka, kb - i) / binom(n, kb))
                .sum();
            let got = hypergeometric_sf(&lf, n, ka, kb, k);
            assert!((got - direct).abs() < 1e-9 * direct.max(1e-300) + 1e-12, "{n} {ka} {kb} {k}: {got} vs {direct}");
        }
    }

    #[test]
    fn stirling_continues_the_table() {
        let small = LnFactorial::new(10);
        let big = LnFactorial::new(100_000);
        for k in [11u64, 50, 1000, 70_000] {
            assert!((small.get(k) - big.get(k)).abs() < 1e-9 * big.get(k), "{k}");
        }
    }

    #[test]
    fn rare_files_changing_together_is_significant() {
        let lf = LnFactorial::new(1000);
        // Two files changed 6 times each in 1000 commits, always together.
        assert!(hypergeometric_sf(&lf, 1000, 6, 6, 6) < 1e-12);
        // Two files each in half the commits, sharing a quarter: what chance predicts.
        assert!(hypergeometric_sf(&lf, 1000, 500, 500, 250) > 0.4);
    }

    #[test]
    fn doa_thresholds() {
        // A creator with one commit and nobody else knows the file.
        assert!(doa(true, 1, 0) > 3.293);
        // Someone with one commit to a file others changed 40 times does not.
        assert!(doa(false, 1, 40) < 3.293);
    }

    #[test]
    fn prefixes() {
        assert!(under("src/a.rs", "src"));
        assert!(under("src/a.rs", "src/"));
        assert!(!under("srcx/a.rs", "src"));
        assert!(under("anything", ""));
    }
}
