use serde_json::Value;
use std::path::Path;
use std::process::Command;

const DAY: i64 = 86_400;

struct Repo {
    dir: tempfile::TempDir,
    /// Author time of the next commit; each commit is a day after the last.
    t: i64,
}

impl Repo {
    fn new() -> Self {
        let r = Repo { dir: tempfile::tempdir().unwrap(), t: 1_704_067_200 };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.name", "Test"]);
        r.git(&["config", "user.email", "test@example.com"]);
        r.git(&["config", "core.autocrlf", "false"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(self.path()).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn write(&self, file: &str, content: &str) {
        let p = self.path().join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn commit(&mut self, author: &str, msg: &str) {
        self.git(&["add", "-A"]);
        let date = format!("{} +0000", self.t);
        self.t += DAY;
        let out = Command::new("git")
            .args(["commit", "-q", "--allow-empty", "-m", msg, "--author", author])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .current_dir(self.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "commit: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_gmap"))
            .args(args)
            .current_dir(self.path())
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        let (ok, out, err) = self.run(&a);
        assert!(ok, "gmap {args:?} failed: {err}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad JSON from {args:?}: {e}\n{out}"))
    }
}

/// Indented code, so files have some complexity and rank as hotspots.
fn code(n: usize, tag: &str) -> String {
    (0..n).map(|i| format!("fn f{i}() {{\n    if x {{\n        y(\"{tag}\");\n    }}\n}}\n")).collect()
}

fn hot_paths(v: &Value) -> Vec<String> {
    v["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
}

fn hot<'a>(v: &'a Value, path: &str) -> &'a Value {
    v["files"].as_array().unwrap().iter().find(|f| f["path"] == path).unwrap_or_else(|| panic!("{path} not in {v}"))
}

const ADA: &str = "Ada Lovelace <ada@example.com>";
const BOB: &str = "Bob Stone <bob@example.com>";
const CY: &str = "Cy Young <cy@example.com>";

#[test]
fn submodules_in_history_are_skipped() {
    let mut sub = Repo::new();
    sub.write("lib.c", "int x;\n");
    sub.commit(ADA, "one");
    sub.write("lib.c", "int x, y;\n");
    sub.commit(ADA, "two");

    let mut r = Repo::new();
    r.write("main.c", &code(3, "a"));
    r.commit(ADA, "start");
    let url = sub.path().to_str().unwrap().replace('\\', "/");
    r.git(&["-c", "protocol.file.allow=always", "submodule", "add", "-q", &url, "vendor/lib"]);
    r.commit(ADA, "add submodule");
    r.git(&["-C", "vendor/lib", "checkout", "-q", "HEAD~1"]);
    r.commit(ADA, "move submodule");
    r.write("main.c", &code(4, "b"));
    r.commit(ADA, "change");

    let v = r.json(&["hotspots"]);
    assert_eq!(hot_paths(&v), vec!["main.c"]);
    assert_eq!(hot(&v, "main.c")["changes"], 2);
}

#[test]
fn renames_keep_their_history() {
    let mut r = Repo::new();
    r.write("src/old.rs", &code(5, "0"));
    r.commit(ADA, "add");
    for i in 1..=3 {
        r.write("src/old.rs", &code(5 + i, "x"));
        r.commit(ADA, "edit");
    }
    r.git(&["mv", "src/old.rs", "src/new.rs"]);
    r.commit(ADA, "rename");
    r.write("src/new.rs", &code(9, "x"));
    r.commit(ADA, "edit again");

    let v = r.json(&["hotspots"]);
    assert_eq!(hot_paths(&v), vec!["src/new.rs"]);
    assert_eq!(hot(&v, "src/new.rs")["changes"], 6);
    assert_eq!(hot(&v, "src/new.rs")["first_change"], "2024-01-01");
}

#[test]
fn merges_are_not_counted_twice() {
    let mut r = Repo::new();
    r.write("a.py", &code(2, "a"));
    r.commit(ADA, "base");
    r.git(&["checkout", "-q", "-b", "side"]);
    r.write("b.py", &code(2, "b"));
    r.commit(BOB, "side work");
    r.git(&["checkout", "-q", "main"]);
    r.write("a.py", &code(3, "a"));
    r.commit(ADA, "main work");
    let date = format!("{} +0000", r.t);
    let out = Command::new("git")
        .args(["merge", "-q", "--no-ff", "-m", "merge", "side"])
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .current_dir(r.path())
        .output()
        .unwrap();
    assert!(out.status.success());

    let v = r.json(&["hotspots"]);
    assert_eq!(v["commits"], 3);
    assert_eq!(hot(&v, "b.py")["changes"], 1);
    assert_eq!(hot(&v, "a.py")["changes"], 2);
}

#[test]
fn truck_factor_of_a_one_person_project_is_one() {
    let mut r = Repo::new();
    for i in 0..6 {
        r.write(&format!("core/f{i}.go"), &code(2, "a"));
    }
    r.commit(ADA, "core");
    for i in 0..2 {
        r.write(&format!("ui/b{i}.go"), &code(2, "b"));
    }
    r.commit(BOB, "ui");
    for i in 0..2 {
        r.write(&format!("cli/c{i}.go"), &code(2, "c"));
    }
    r.commit(CY, "cli");

    let v = r.json(&["owners"]);
    assert_eq!(v["truck_factor"], 1);
    assert_eq!(v["key_people"], serde_json::json!(["Ada Lovelace"]));
    assert_eq!(v["files"], 10);
}

#[test]
fn truck_factor_counts_people_until_half_the_files_are_orphaned() {
    let mut r = Repo::new();
    for (who, dir) in [(ADA, "a"), (BOB, "b"), (CY, "c")] {
        for i in 0..4 {
            r.write(&format!("{dir}/f{i}.ts"), &code(2, dir));
        }
        r.commit(who, dir);
    }
    // Ada and Bob each know a third; after both leave, Cy still knows a third.
    let v = r.json(&["owners"]);
    assert_eq!(v["truck_factor"], 2);
}

#[test]
fn people_who_left_leave_orphans() {
    let mut r = Repo::new();
    r.write("old/legacy.c", &code(4, "old"));
    r.commit(BOB, "legacy");
    r.t += 3 * 365 * DAY;
    r.write("new/fresh.c", &code(4, "new"));
    r.commit(ADA, "fresh");

    let v = r.json(&["owners"]);
    let orphans: Vec<&str> =
        v["orphaned_files"].as_array().unwrap().iter().map(|o| o["path"].as_str().unwrap()).collect();
    assert_eq!(orphans, vec!["old/legacy.c"]);
    let bob = v["people"].as_array().unwrap().iter().find(|p| p["name"] == "Bob Stone").unwrap();
    assert_eq!(bob["active"], false);
}

#[test]
fn bots_are_left_out_unless_asked() {
    let mut r = Repo::new();
    r.write("app.js", &code(3, "a"));
    r.commit(ADA, "app");
    for i in 0..5 {
        r.write("app.js", &code(3 + i, "bump"));
        r.commit("dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>", "bump");
    }
    let v = r.json(&["hotspots"]);
    assert_eq!(hot(&v, "app.js")["changes"], 1);
    let v = r.json(&["hotspots", "--bots"]);
    assert_eq!(hot(&v, "app.js")["changes"], 6);
}

#[test]
fn one_person_with_two_emails_is_one_person() {
    let mut r = Repo::new();
    r.write("x.rb", &code(2, "a"));
    r.commit("Ada Lovelace <ada@work.example>", "one");
    r.write("x.rb", &code(3, "a"));
    r.commit("Ada Lovelace <ada@home.example>", "two");
    r.write("y.rb", &code(3, "b"));
    r.commit("ada <ADA@HOME.EXAMPLE>", "three");
    let v = r.json(&["owners"]);
    assert_eq!(v["people"].as_array().unwrap().len(), 1);
}

#[test]
fn mailmap_is_honoured() {
    let mut r = Repo::new();
    r.write(".mailmap", "Ada Lovelace <ada@example.com> <a@old.example>\n");
    r.write("x.rb", &code(2, "a"));
    r.commit("Old Name <a@old.example>", "one");
    let v = r.json(&["owners"]);
    assert_eq!(v["people"][0]["name"], "Ada Lovelace");
}

#[test]
fn blame_ignore_revs_are_skipped() {
    let mut r = Repo::new();
    r.write("m.py", &code(3, "a"));
    r.commit(ADA, "start");
    r.write("m.py", &code(3, "a").replace("    ", "  "));
    r.commit(BOB, "reformat everything");
    let rev = r.git(&["rev-parse", "HEAD"]);
    r.write(".git-blame-ignore-revs", &format!("# formatting\n{rev}"));
    r.commit(ADA, "ignore the reformat");
    let v = r.json(&["hotspots"]);
    assert_eq!(hot(&v, "m.py")["changes"], 1);
}

#[test]
fn coupling_finds_files_that_change_together() {
    let mut r = Repo::new();
    r.write("parser.c", &code(2, "0"));
    r.write("lexer.c", &code(2, "0"));
    r.write("util.c", &code(2, "0"));
    r.commit(ADA, "start");
    for i in 1..=6 {
        r.write("parser.c", &code(2 + i, "p"));
        r.write("lexer.c", &code(2 + i, "l"));
        r.commit(ADA, "grammar");
        for j in 0..3 {
            r.write("util.c", &code(2 + i * 3 + j, "u"));
            r.commit(ADA, "util");
        }
    }
    let v = r.json(&["coupling"]);
    let pairs = v["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1, "{v}");
    assert_eq!(pairs[0]["shared_commits"], 6);
    assert_eq!(pairs[0]["degree"], 1.0);
    let names = [pairs[0]["a"].as_str().unwrap(), pairs[0]["b"].as_str().unwrap()];
    assert!(names.contains(&"parser.c") && names.contains(&"lexer.c"));
}

#[test]
fn files_created_together_are_not_coupled() {
    let mut r = Repo::new();
    for k in 0..5 {
        r.write(&format!("a{k}.c"), &code(2, "a"));
        r.write(&format!("b{k}.c"), &code(2, "b"));
        r.commit(ADA, "two new files");
    }
    let v = r.json(&["coupling"]);
    assert_eq!(v["pairs"].as_array().unwrap().len(), 0);
}

#[test]
fn time_window() {
    let mut r = Repo::new();
    r.write("a.rs", &code(2, "a"));
    r.commit(ADA, "january"); // 2024-01-01
    r.t += 60 * DAY;
    r.write("a.rs", &code(3, "a"));
    r.commit(ADA, "march");
    let v = r.json(&["hotspots", "--since", "2024-02-01"]);
    assert_eq!(v["commits"], 1);
    let v = r.json(&["hotspots", "--until", "2024-02-01"]);
    assert_eq!(v["commits"], 1);
    let v = r.json(&["hotspots", "--since", "1m"]);
    assert_eq!(v["commits"], 1);
}

#[test]
fn only_code_unless_all() {
    let mut r = Repo::new();
    r.write("src/lib.rs", &code(3, "a"));
    r.write("Cargo.lock", "[[package]]\nname = \"x\"\n");
    r.write("README.md", "# hi\n\n- a\n  - b\n");
    r.write("gen/out.rs", &code(2, "g"));
    r.write(".gitattributes", "gen/** linguist-generated\n");
    r.commit(ADA, "all");
    let v = r.json(&["hotspots"]);
    assert_eq!(hot_paths(&v), vec!["src/lib.rs"]);
    let v = r.json(&["hotspots", "--all"]);
    let mut p = hot_paths(&v);
    p.sort();
    assert!(
        p.contains(&"Cargo.lock".to_string())
            && p.contains(&"README.md".to_string())
            && p.contains(&"gen/out.rs".to_string()),
        "{p:?}"
    );
    let v = r.json(&["hotspots", "--exclude", "src/"]);
    assert!(hot_paths(&v).is_empty());
}

#[test]
fn path_argument_narrows_everything() {
    let mut r = Repo::new();
    r.write("a/x.c", &code(2, "a"));
    r.write("b/y.c", &code(2, "b"));
    r.commit(ADA, "both");
    let v = r.json(&["hotspots", "a"]);
    assert_eq!(hot_paths(&v), vec!["a/x.c"]);
}

#[test]
fn cache_gives_the_same_answers() {
    let mut r = Repo::new();
    for i in 0..12 {
        r.write(&format!("m{}.c", i % 4), &code(2 + i, "c"));
        if i % 5 == 4 {
            let f = format!("m{}.c", i % 4);
            r.git(&["mv", &f, &format!("renamed{i}.c")]);
        }
        r.commit(if i % 2 == 0 { ADA } else { BOB }, "work");
    }
    let cold = r.json(&["hotspots", "--no-cache"]);
    let first = r.json(&["hotspots"]);
    assert!(r.path().join(".git/gmap/cache").exists());
    let warm = r.json(&["hotspots"]);
    assert_eq!(cold, first);
    assert_eq!(cold, warm);
    // The cache lives in .git, so the working tree stays clean.
    assert_eq!(r.git(&["status", "--porcelain"]), "");
    // New commits after the cache was written are picked up.
    r.write("m0.c", &code(40, "new"));
    r.commit(ADA, "after the cache");
    let v = r.json(&["hotspots"]);
    assert_eq!(v["commits"], 13);
}

#[test]
fn line_counts_match_git() {
    let mut r = Repo::new();
    r.write("t.txt", "a\nb\nc\nd\n");
    r.write("u.c", &code(3, "u"));
    r.commit(ADA, "one");
    r.write("t.txt", "a\nB\nc\nd\ne\nf\n");
    r.write("u.c", &code(2, "u"));
    r.commit(ADA, "two");
    let (ok, out, _) = r.run(&["export", "--all"]);
    assert!(ok);
    let rows: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let numstat = r.git(&["log", "--numstat", "--format=", "--diff-algorithm=histogram"]);
    let mut git: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    for line in numstat.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let e = git.entry(f[2].to_string()).or_default();
        e.0 += f[0].parse::<u64>().unwrap();
        e.1 += f[1].parse::<u64>().unwrap();
    }
    let mut mine: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    for row in &rows {
        let e = mine.entry(row["path"].as_str().unwrap().to_string()).or_default();
        e.0 += row["added"].as_u64().unwrap();
        e.1 += row["deleted"].as_u64().unwrap();
    }
    assert_eq!(mine, git);
}

#[test]
fn binary_and_odd_files_do_not_break_anything() {
    let mut r = Repo::new();
    std::fs::write(r.path().join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 1, 2, 3]).unwrap();
    std::fs::write(r.path().join("latin1.c"), b"/* caf\xe9 */\nint x;\n").unwrap();
    r.write("empty.c", "");
    r.write("no_newline.c", "int y;");
    r.commit(ADA, "odd files");
    std::fs::write(r.path().join("logo.png"), [0x89u8, b'P', b'N', b'G', 0, 9, 9]).unwrap();
    r.commit(ADA, "new logo");
    for cmd in ["hotspots", "coupling", "owners", "activity", "churn"] {
        r.json(&[cmd]);
    }
    let (ok, _, err) = r.run(&["export", "--all"]);
    assert!(ok, "{err}");
}

#[test]
fn text_reports_render() {
    let mut r = Repo::new();
    r.write("src/a.rs", &code(3, "a"));
    r.commit(ADA, "a");
    r.write("src/a.rs", &code(4, "a"));
    r.commit(BOB, "b");
    for cmd in ["hotspots", "coupling", "owners", "activity", "churn"] {
        let (ok, out, err) = r.run(&[cmd]);
        assert!(ok, "{cmd}: {err}");
        assert!(out.contains("2 commits by 2 people"), "{cmd}:\n{out}");
    }
}

#[test]
fn friendly_errors() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_gmap")).args(["hotspots"]).current_dir(dir.path()).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));

    let r = Repo::new();
    let (ok, _, err) = r.run(&["hotspots"]);
    assert!(!ok);
    assert!(err.contains("no commits yet"), "{err}");

    let (ok, _, err) = r.run(&["hotspots", "--since", "someday"]);
    assert!(!ok);
    assert!(err.starts_with("gmap: "), "{err}");

    // The map needs a terminal; a pipe gets a pointer to the text commands.
    let mut r = Repo::new();
    r.write("a.c", "int x;\n");
    r.commit(ADA, "a");
    let (ok, _, err) = r.run(&[]);
    assert!(!ok);
    assert!(err.contains("gmap hotspots"), "{err}");
}
