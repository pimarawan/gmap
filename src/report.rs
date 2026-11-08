use crate::analysis::{self, Analysis, Settings};
use crate::filter::PathFilter;
use crate::history::History;
use crate::paint::{self, Rgb};
use crate::time::{self, DAY, YEAR};
use serde_json::json;
use std::collections::HashMap;
use std::io::{self, Write};

pub struct Ctx<'a> {
    pub h: &'a History,
    pub a: &'a Analysis,
    pub filter: &'a PathFilter,
    pub s: &'a Settings,
    pub color: bool,
    /// Terminal width, or `usize::MAX` when writing to a pipe.
    pub width: usize,
    pub limit: usize,
}

impl Ctx<'_> {
    fn fg(&self, c: Rgb, s: &str) -> String {
        if self.color {
            format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", c.0, c.1, c.2)
        } else {
            s.to_string()
        }
    }
    fn dim(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn path(&self, p: u32) -> &str {
        &self.h.paths[p as usize]
    }
    fn name(&self, p: u32) -> &str {
        &self.h.people[p as usize].name
    }
    fn gone(&self, p: u32) -> bool {
        self.a.person(p).is_some_and(|p| !p.active)
    }
    /// Columns left for a final, variable-width column.
    fn rest(&self, used: usize) -> usize {
        if self.width == usize::MAX {
            usize::MAX / 2
        } else {
            self.width.saturating_sub(used).max(12)
        }
    }
    fn window(&self) -> String {
        match (self.s.since, self.s.until) {
            (Some(s), Some(u)) => format!("from {} to {}", time::date(s), time::date(u)),
            (Some(s), None) => format!("since {}", time::date(s)),
            (None, Some(u)) => format!("before {}", time::date(u)),
            (None, None) => String::new(),
        }
    }

    fn header(&self, out: &mut impl Write) -> io::Result<()> {
        let h = self.h;
        let a = self.a;
        let (first, last) = a
            .commits
            .iter()
            .map(|&c| h.commits[c as usize].time)
            .fold((i64::MAX, i64::MIN), |(lo, hi), t| (lo.min(t), hi.max(t)));
        let span =
            if first <= last { format!(", {} to {}", time::date(first), time::date(last)) } else { String::new() };
        writeln!(
            out,
            "{}  {}  {} commits by {} {}{span}",
            self.bold(&h.name),
            h.branch,
            paint::thousands(a.commits.len() as u64),
            paint::thousands(a.people.len() as u64),
            if a.people.len() == 1 { "person" } else { "people" },
        )?;
        let kind = if self.s.code_only { "source files" } else { "files" };
        let mut notes = vec![format!("{} {kind}", paint::thousands(a.files.len() as u64))];
        if a.excluded > 0 {
            notes.push(format!("{} others left out (--all keeps them)", paint::thousands(a.excluded as u64)));
        }
        let w = self.window();
        if !w.is_empty() {
            notes.push(w);
        }
        if let Some(au) = &self.s.author {
            notes.push(format!("author ~ {au}"));
        }
        if h.partial > 0 {
            notes.push(format!("{} commits unreadable (shallow clone?)", h.partial));
        }
        writeln!(out, "{}\n", self.dim(&notes.join(", ")))
    }
}

fn base_json(c: &Ctx) -> serde_json::Value {
    json!({
        "repository": c.h.name,
        "revision": c.h.branch,
        "tip": c.h.tip.to_string(),
        "commits": c.a.commits.len(),
        "since": c.s.since.map(time::date),
        "until": c.s.until.map(time::date),
    })
}

fn emit(out: &mut impl Write, v: serde_json::Value) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, &v)?;
    writeln!(out)
}

pub fn hotspots(c: &Ctx, json: bool, out: &mut impl Write) -> io::Result<()> {
    let a = c.a;
    let ranked = a.hotspots();
    let shown = &ranked[..ranked.len().min(c.limit)];
    if json {
        let mut v = base_json(c);
        v["files"] = shown
            .iter()
            .map(|&i| {
                let f = &a.files[i];
                json!({
                    "path": c.path(f.path),
                    "score": (f.score * 100.0).round() / 100.0,
                    "changes": f.revs,
                    "weighted_changes": (f.weight * 100.0).round() / 100.0,
                    "lines": f.lines,
                    "complexity": f.complexity,
                    "added": f.added,
                    "deleted": f.deleted,
                    "first_change": time::date(f.first),
                    "last_change": time::date(f.last),
                    "created_by": f.creator.map(|p| c.name(p)),
                    "owners": f.owners.iter().map(|&p| json!({"name": c.name(p), "active": !c.gone(p)})).collect::<Vec<_>>(),
                })
            })
            .collect();
        return emit(out, v);
    }
    c.header(out)?;
    if shown.is_empty() {
        return writeln!(out, "No file changed in this window.");
    }
    let max = a.files[shown[0]].score.max(1e-9);
    writeln!(out, "{}", c.dim("   #  score       changes  last 12 months  lines   owner               file"))?;
    let mut any_gone = false;
    for (rank, &i) in shown.iter().enumerate() {
        let f = &a.files[i];
        let t = f.score / max;
        let trend = analysis::histogram(c.h, &f.commits, a.now - YEAR, a.now, 12);
        let owner = match f.owners.first() {
            Some(&p) if c.gone(p) => {
                any_gone = true;
                c.dim(&paint::fit(&format!("{}†", paint::head(c.name(p), 17)), 18))
            }
            Some(&p) => paint::fit(c.name(p), 18),
            None => c.dim(&paint::fit("nobody", 18)),
        };
        writeln!(
            out,
            "{:>4}  {}  {:>7}  {}  {:>6}   {}  {}",
            rank + 1,
            c.fg(paint::heat(t), &paint::bar(t, 10)),
            paint::thousands(f.revs as u64),
            c.fg((120, 170, 255), &format!("{:<14}", paint::spark(&trend))),
            paint::thousands(f.lines as u64),
            owner,
            paint::tail(c.path(f.path), c.rest(73)),
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        c.dim(&format!(
            "score = changes, each halved for every {} of age, x ln(1 + indentation complexity)",
            time::span(c.s.half_life)
        ))
    )?;
    if any_gone {
        writeln!(out, "{}", c.dim(&format!("† no commit in the last {}", time::span(c.s.inactive))))?;
    }
    Ok(())
}

pub fn coupling(c: &Ctx, cp: &analysis::Coupling, across: bool, json: bool, out: &mut impl Write) -> io::Result<()> {
    let a = c.a;
    let dir = |i: usize| c.path(a.files[i].path).rsplit_once('/').map_or("", |(d, _)| d);
    let pairs: Vec<&analysis::Pair> =
        cp.pairs.iter().filter(|p| !across || dir(p.a) != dir(p.b)).take(c.limit).collect();
    if json {
        let mut v = base_json(c);
        v["tested_pairs"] = json!(cp.tested);
        v["commits_considered"] = json!(cp.commits);
        v["pairs"] = pairs
            .iter()
            .map(|p| {
                json!({
                    "a": c.path(a.files[p.a].path),
                    "b": c.path(a.files[p.b].path),
                    "shared_commits": p.shared,
                    "a_commits": p.revs_a,
                    "b_commits": p.revs_b,
                    "degree": (p.degree * 1000.0).round() / 1000.0,
                    "p_value": p.p,
                })
            })
            .collect();
        return emit(out, v);
    }
    c.header(out)?;
    if pairs.is_empty() {
        writeln!(out, "No pair of files changes together more than chance would explain.")?;
    } else {
        writeln!(out, "{}", c.dim("  together  shared  files"))?;
        for p in &pairs {
            let fa = c.path(a.files[p.a].path);
            let fb = c.path(a.files[p.b].path);
            let mark = if dir(p.a) != dir(p.b) { c.fg((255, 170, 90), "↔") } else { c.dim("↔") };
            writeln!(
                out,
                "  {}  {:>6}  {}\n  {}  {}  {} {}",
                c.fg(paint::heat(p.degree), &format!("{:>3.0}% {}", p.degree * 100.0, paint::bar(p.degree, 3))),
                p.shared,
                paint::tail(fa, c.rest(20)),
                " ".repeat(8),
                " ".repeat(6),
                mark,
                paint::tail(fb, c.rest(22)),
            )?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        c.dim(&format!(
            "together = shared commits / average commits of the two. Tested {} pairs over {} commits of at most {} files;\na pair is shown only if that much overlap has less than a 1 in {} chance of happening by accident.",
            paint::thousands(cp.tested as u64),
            paint::thousands(cp.commits as u64),
            c.s.max_changeset,
            paint::thousands((20.0 * cp.tested.max(1) as f64) as u64),
        ))
    )?;
    if across {
        writeln!(out, "{}", c.dim("only pairs in different directories"))?;
    }
    Ok(())
}

pub fn owners(c: &Ctx, json: bool, out: &mut impl Write) -> io::Result<()> {
    let a = c.a;
    let all: Vec<usize> = (0..a.files.len()).collect();
    let truck = analysis::truck_factor(a, &all);
    let orphans = analysis::orphans(a, &all);
    let mut people: Vec<&analysis::PersonStat> = a.people.iter().collect();
    people.sort_by(|x, y| y.owns.cmp(&x.owns).then(y.commits.cmp(&x.commits)));
    if json {
        let mut v = base_json(c);
        v["truck_factor"] = json!(truck.factor);
        v["key_people"] = truck.key.iter().map(|&p| c.name(p)).collect();
        v["coverage_after_each_leaves"] = json!(truck.coverage);
        v["files"] = json!(truck.files);
        v["people"] = people
            .iter()
            .take(c.limit)
            .map(|p| {
                let person = &c.h.people[p.person as usize];
                json!({
                    "name": person.name,
                    "email": person.email,
                    "owns": p.owns,
                    "commits": p.commits,
                    "added": p.added,
                    "deleted": p.deleted,
                    "first_commit": time::date(p.first),
                    "last_commit": time::date(p.last),
                    "active": p.active,
                })
            })
            .collect();
        v["orphaned_files"] = orphans
            .iter()
            .map(|&i| json!({"path": c.path(a.files[i].path), "owners": a.files[i].owners.iter().map(|&p| c.name(p)).collect::<Vec<_>>()}))
            .collect();
        return emit(out, v);
    }
    c.header(out)?;
    if truck.files == 0 {
        return writeln!(out, "No files to look at.");
    }
    let names: Vec<String> = truck.key.iter().map(|&p| c.name(p).to_string()).collect();
    writeln!(
        out,
        "  {} {}",
        c.bold("truck factor"),
        c.fg(paint::truck_color(truck.factor), &c.bold(&truck.factor.to_string()))
    )?;
    if truck.factor == 0 {
        writeln!(out, "  More than half the files already have nobody who knows them well.")?;
    } else {
        let left = truck.coverage.last().copied().unwrap_or(0.0);
        writeln!(
            out,
            "  If {} left, {:.0}% of {} files would have nobody left who knows them.",
            join_names(&names),
            (1.0 - left) * 100.0,
            paint::thousands(truck.files as u64)
        )?;
    }
    writeln!(out)?;
    writeln!(out, "{}", c.dim("   owns  commits   added  deleted  last commit       person"))?;
    for p in people.iter().take(c.limit) {
        let person = &c.h.people[p.person as usize];
        let ago = ago(a.now, p.last);
        let line = format!(
            "  {:>5}  {:>7}  {:>6}  {:>7}  {:<16}  {}",
            paint::thousands(p.owns as u64),
            paint::thousands(p.commits as u64),
            paint::short(p.added),
            paint::short(p.deleted),
            ago,
            paint::head(&person.name, c.rest(52)),
        );
        writeln!(out, "{}", if p.active { line } else { c.dim(&format!("{line}  (gone)")) })?;
    }
    if !orphans.is_empty() {
        writeln!(out)?;
        writeln!(
            out,
            "  {} files are known only by people with no commit in the last {}:",
            c.bold(&paint::thousands(orphans.len() as u64)),
            time::span(c.s.inactive)
        )?;
        let mut orphans = orphans;
        orphans.sort_by(|&x, &y| a.files[y].lines.cmp(&a.files[x].lines));
        for &i in orphans.iter().take(c.limit.min(15)) {
            let f = &a.files[i];
            let who: Vec<String> = f.owners.iter().map(|&p| c.name(p).to_string()).collect();
            writeln!(
                out,
                "  {:>7}  {}  {}",
                paint::thousands(f.lines as u64),
                paint::fit(&who.join(", "), 22),
                paint::tail(c.path(f.path), c.rest(35))
            )?;
        }
        if orphans.len() > c.limit.min(15) {
            writeln!(out, "  {}", c.dim(&format!("and {} more", orphans.len() - c.limit.min(15))))?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        c.dim("owns = files where this person's degree of authorship (Fritz et al.) is high enough to count as knowing it.\ntruck factor = how many of the top owners could leave before over half the files have no owner (Avelino et al.).")
    )
}

fn join_names(n: &[String]) -> String {
    match n.len() {
        0 => String::new(),
        1 => n[0].clone(),
        2 => format!("{} and {}", n[0], n[1]),
        k if k <= 5 => format!("{} and {}", n[..k - 1].join(", "), n[k - 1]),
        k => format!("{} and {} others", n[..4].join(", "), k - 4),
    }
}

pub fn activity(c: &Ctx, json: bool, out: &mut impl Write) -> io::Result<()> {
    let act = analysis::activity(c.h, &c.a.commits);
    if json {
        let mut v = base_json(c);
        v["days"] = act.days.iter().map(|&(d, n)| json!({"date": time::date(d * DAY), "commits": n})).collect();
        v["weekday_hour"] = json!(act.punch);
        let mut months: Vec<(String, u32)> = Vec::new();
        for &(d, n) in &act.days {
            let m = time::month(d * DAY);
            match months.last_mut() {
                Some((lm, k)) if *lm == m => *k += n,
                _ => months.push((m, n)),
            }
        }
        v["months"] = months.into_iter().map(|(m, n)| json!({"month": m, "commits": n})).collect();
        return emit(out, v);
    }
    c.header(out)?;
    if act.days.is_empty() {
        return writeln!(out, "No commits in this window.");
    }
    let days: HashMap<i64, u32> = act.days.iter().copied().collect();

    // A year of days, Monday at the top, ending on the week of the newest commit.
    let last_day = act.days.last().unwrap().0;
    let weeks = if c.width == usize::MAX { 53 } else { ((c.width.saturating_sub(6)) / 2).clamp(8, 53) };
    let start = last_day - time::weekday(last_day) as i64 - (weeks as i64 - 1) * 7;
    let mut counts: Vec<u32> = (0..weeks * 7).map(|k| days.get(&(start + k as i64)).copied().unwrap_or(0)).collect();
    let levels = quantiles(&mut counts.clone());
    let mut label = String::from("      ");
    let mut k = 0;
    while k < weeks {
        let (_, m, d) = time::civil_from_days(start + k as i64 * 7);
        if d <= 7 || k == 0 {
            let name = time::MONTHS[m as usize - 1];
            label.push_str(name);
            label.push(' ');
            k += 2;
            if k > weeks {
                break;
            }
        } else {
            label.push_str("  ");
            k += 1;
        }
    }
    writeln!(out, "{}", c.dim(label.trim_end()))?;
    for wd in 0..7 {
        let name = ["Mon", "", "Wed", "", "Fri", "", "Sun"][wd];
        let mut line = format!("  {name:<4}");
        for w in 0..weeks {
            let d = start + (w * 7 + wd) as i64;
            if d > last_day {
                break;
            }
            let n = counts[w * 7 + wd];
            let lvl = level(&levels, n);
            line.push_str(&if c.color {
                c.fg(paint::GREENS[lvl], "■ ")
            } else {
                format!("{} ", [' ', '░', '▒', '▓', '█'][lvl])
            });
        }
        writeln!(out, "{line}")?;
    }
    counts.clear();
    writeln!(out)?;

    // When in the week people commit, in their own time zone.
    let max = act.punch.iter().flatten().copied().max().unwrap_or(1).max(1);
    writeln!(out, "{}", c.dim("       0     3     6     9     12    15    18    21    hour (author's time)"))?;
    for (wd, row) in act.punch.iter().enumerate() {
        let name = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][wd];
        let mut line = format!("  {name}  ");
        for &n in row {
            let t = n as f64 / max as f64;
            line.push_str(&c.fg(paint::heat(0.25 + 0.75 * t), &format!("{} ", paint::dot(t))));
        }
        writeln!(out, "{line}")?;
    }
    writeln!(out)?;

    let mut months: Vec<(i64, u32)> = Vec::new();
    for &(d, n) in &act.days {
        let (y, m, _) = time::civil_from_days(d);
        let key = y * 12 + m as i64 - 1;
        match months.last_mut() {
            Some((k, v)) if *k == key => *v += n,
            _ => months.push((key, n)),
        }
    }
    let (m0, m1) = (months[0].0, months.last().unwrap().0);
    let mut series = vec![0u32; (m1 - m0 + 1) as usize];
    for (k, n) in months {
        series[(k - m0) as usize] = n;
    }
    let width = if c.width == usize::MAX { 100 } else { c.width.saturating_sub(4).max(10) };
    let series = resample(&series, width);
    writeln!(out, "  {}", c.fg(paint::GREENS[3], &paint::spark(&series)))?;
    let (y0, y1) = (m0.div_euclid(12), m1.div_euclid(12));
    let left = format!("{}-{:02}", y0, m0.rem_euclid(12) + 1);
    let right = format!("{}-{:02}", y1, m1.rem_euclid(12) + 1);
    let gap = series.len().saturating_sub(left.len() + right.len());
    writeln!(out, "  {}", c.dim(&format!("{left}{}{right}  commits per month", " ".repeat(gap))))?;

    let streak = longest_streak(&act.days);
    let busiest = act.days.iter().max_by_key(|d| (d.1, -d.0)).unwrap();
    writeln!(out)?;
    writeln!(
        out,
        "  {} active days. Busiest: {} with {} commits. Longest streak: {} days{}.",
        paint::thousands(act.days.len() as u64),
        time::date(busiest.0 * DAY),
        busiest.1,
        streak.0,
        if streak.0 > 1 { format!(", ending {}", time::date(streak.1 * DAY)) } else { String::new() }
    )
}

/// Split nonzero counts at their quartiles, so one huge day does not wash out the rest.
fn quantiles(v: &mut [u32]) -> [u32; 3] {
    let mut nz: Vec<u32> = v.iter().copied().filter(|&n| n > 0).collect();
    if nz.is_empty() {
        return [1, 1, 1];
    }
    nz.sort_unstable();
    let q = |f: f64| nz[((nz.len() - 1) as f64 * f).round() as usize];
    [q(0.25), q(0.5), q(0.75)]
}

fn level(q: &[u32; 3], n: u32) -> usize {
    match n {
        0 => 0,
        n if n <= q[0] => 1,
        n if n <= q[1] => 2,
        n if n <= q[2] => 3,
        _ => 4,
    }
}

/// Sum adjacent values so the series fits in `width` columns.
pub fn resample(v: &[u32], width: usize) -> Vec<u32> {
    if v.len() <= width {
        return v.to_vec();
    }
    let per = v.len().div_ceil(width);
    v.chunks(per).map(|c| c.iter().sum()).collect()
}

/// (length, last day) of the longest run of consecutive days with commits.
pub fn longest_streak(days: &[(i64, u32)]) -> (usize, i64) {
    let mut best = (0, 0);
    let mut run = 0;
    let mut prev = i64::MIN;
    for &(d, _) in days {
        run = if d == prev + 1 { run + 1 } else { 1 };
        prev = d;
        if run > best.0 {
            best = (run, d);
        }
    }
    best
}

pub fn churn(c: &Ctx, depth: Option<usize>, json: bool, out: &mut impl Write) -> io::Result<()> {
    struct Row {
        added: u64,
        deleted: u64,
        commits: u32,
        authors: HashMap<u32, ()>,
        last: u32,
    }
    let h = c.h;
    let mut rows: HashMap<String, Row> = HashMap::new();
    for &ci in &c.a.commits {
        let cm = &h.commits[ci as usize];
        for ch in h.changes(cm) {
            let path = &h.paths[ch.path as usize];
            if c.filter.excludes(path) || c.s.prefix.as_ref().is_some_and(|p| !analysis::under(path, p)) {
                continue;
            }
            let key = match depth {
                Some(d) => path.split('/').take(d.max(1)).collect::<Vec<_>>().join("/"),
                None => path.clone(),
            };
            let r = rows.entry(key).or_insert(Row {
                added: 0,
                deleted: 0,
                commits: 0,
                authors: HashMap::new(),
                last: u32::MAX,
            });
            r.added += ch.added as u64;
            r.deleted += ch.deleted as u64;
            if r.last != ci {
                r.commits += 1;
                r.last = ci;
            }
            r.authors.insert(cm.author, ());
        }
    }
    let mut rows: Vec<(String, Row)> = rows.into_iter().collect();
    rows.sort_by(|x, y| (y.1.added + y.1.deleted).cmp(&(x.1.added + x.1.deleted)).then(x.0.cmp(&y.0)));
    rows.truncate(c.limit);
    if json {
        let mut v = base_json(c);
        v["entries"] = rows
            .iter()
            .map(|(p, r)| json!({"path": p, "added": r.added, "deleted": r.deleted, "commits": r.commits, "authors": r.authors.len()}))
            .collect();
        return emit(out, v);
    }
    c.header(out)?;
    writeln!(out, "{}", c.dim("     added   deleted  commits  authors  path"))?;
    for (p, r) in &rows {
        writeln!(
            out,
            "  {}  {}  {:>7}  {:>7}  {}",
            c.fg((90, 200, 120), &format!("{:>8}", format!("+{}", paint::short(r.added)))),
            c.fg((235, 100, 100), &format!("{:>8}", format!("-{}", paint::short(r.deleted)))),
            paint::thousands(r.commits as u64),
            r.authors.len(),
            paint::tail(p, c.rest(42)),
        )?;
    }
    Ok(())
}

/// Every change as one JSON object per line.
pub fn export(c: &Ctx, out: &mut impl Write) -> io::Result<()> {
    let h = c.h;
    for &ci in &c.a.commits {
        let cm = &h.commits[ci as usize];
        let who = &h.people[cm.author as usize];
        for ch in h.changes(cm) {
            let path = &h.paths[ch.path as usize];
            if c.filter.excludes(path) || c.s.prefix.as_ref().is_some_and(|p| !analysis::under(path, p)) {
                continue;
            }
            let v = json!({
                "commit": cm.id.to_string(),
                "time": cm.time,
                "tz_offset": cm.tz,
                "author": who.name,
                "email": who.email,
                "path": path,
                "kind": format!("{:?}", ch.kind).to_lowercase(),
                "added": ch.added,
                "deleted": ch.deleted,
                "binary": ch.binary,
            });
            serde_json::to_writer(&mut *out, &v)?;
            writeln!(out)?;
        }
    }
    Ok(())
}

/// "today", "3 weeks ago"
pub fn ago(now: i64, t: i64) -> String {
    if now - t < DAY {
        "today".into()
    } else {
        format!("{} ago", time::span(now - t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaks() {
        assert_eq!(longest_streak(&[(1, 1), (2, 1), (3, 5), (7, 1), (8, 1)]), (3, 3));
        assert_eq!(longest_streak(&[]), (0, 0));
    }

    #[test]
    fn resampling() {
        assert_eq!(resample(&[1, 2, 3, 4, 5], 10), vec![1, 2, 3, 4, 5]);
        assert_eq!(resample(&[1, 2, 3, 4, 5], 3), vec![3, 7, 5]);
    }

    #[test]
    fn levels() {
        let q = quantiles(&mut [0, 1, 2, 3, 4, 100]);
        assert_eq!(level(&q, 0), 0);
        assert_eq!(level(&q, 100), 4);
        assert!(level(&q, 2) < level(&q, 4));
    }
}
