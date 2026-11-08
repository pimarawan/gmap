use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

const DEFAULT_EXCLUDES: &[&str] = &[
    "*.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "bun.lockb",
    "go.sum",
    "packages.lock.json",
    "gradle.lockfile",
    "vendor/",
    "node_modules/",
    "third_party/",
    "third-party/",
    "dist/",
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.snap",
    "*.pb.go",
    "*_pb2.py",
    "*.generated.*",
    "*.g.dart",
    "*.designer.cs",
];

pub struct PathFilter {
    set: GlobSet,
}

impl PathFilter {
    pub fn new(defaults: bool, user: &[String], attributes: &[(String, String)]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        let add = |dir: &str, pat: &str, b: &mut GlobSetBuilder| -> Result<()> {
            for g in translate(dir, pat) {
                let glob = GlobBuilder::new(&g)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .build()
                    .with_context(|| format!("bad exclude pattern '{pat}'"))?;
                b.add(glob);
            }
            Ok(())
        };
        if defaults {
            for p in DEFAULT_EXCLUDES {
                add("", p, &mut b)?;
            }
            for (dir, pat) in attributes {
                add(dir, pat, &mut b)?;
            }
        }
        for p in user {
            add("", p, &mut b)?;
        }
        Ok(Self { set: b.build()? })
    }

    pub fn excludes(&self, path: &str) -> bool {
        self.set.is_match(path)
    }
}

/// Turn a gitignore-style pattern, found in directory `dir`, into globs over
/// full paths. A pattern without a slash matches a name at any depth; a
/// trailing slash, or any match on a directory name, takes everything under it.
fn translate(dir: &str, pat: &str) -> Vec<String> {
    let anchored = pat.starts_with('/') || pat.trim_end_matches('/').contains('/');
    let body = pat.trim_start_matches('/').trim_end_matches('/');
    let base = match (dir.is_empty(), anchored) {
        (true, true) => body.to_string(),
        (true, false) => format!("**/{body}"),
        (false, true) => format!("{dir}/{body}"),
        (false, false) => format!("{dir}/**/{body}"),
    };
    if pat.ends_with('/') {
        vec![format!("{base}/**")]
    } else {
        vec![format!("{base}/**"), base]
    }
}

/// Patterns from one `.gitattributes` file that mark paths as not hand-written.
pub fn generated_patterns(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(pat) = words.next() else { continue };
        if pat.starts_with('!') || pat.starts_with('[') {
            continue; // macro definitions and negations are not path patterns we need
        }
        let marked = words.any(|a| {
            matches!(
                a,
                "linguist-generated"
                    | "linguist-generated=true"
                    | "linguist-vendored"
                    | "linguist-vendored=true"
                    | "-diff"
                    | "binary"
            )
        });
        if marked {
            out.push(pat.to_string());
        }
    }
    out
}

/// Extensions of files people program in. Docs, configuration and data are
/// real changes too, but a README is not where bugs hide.
const CODE: &[&str] = &[
    "ada", "adb", "ads", "asm", "astro", "bash", "c", "cc", "cjs", "cl", "clj", "cljc", "cljs", "cmake", "coffee",
    "cpp", "cr", "cs", "css", "cts", "cu", "cuh", "cxx", "d", "dart", "el", "elm", "erl", "ex", "exs", "f", "f90",
    "f95", "fish", "for", "fs", "fsi", "fsx", "gd", "gleam", "glsl", "go", "gradle", "graphql", "groovy", "h", "hh",
    "hlsl", "hpp", "hrl", "hs", "htm", "html", "hxx", "inl", "java", "jl", "js", "jsx", "kt", "kts", "less", "lhs",
    "lisp", "lua", "m", "metal", "mjs", "ml", "mli", "mm", "mts", "nim", "nix", "odin", "pas", "php", "pl", "pm",
    "proto", "ps1", "psm1", "purs", "py", "pyi", "pyx", "r", "rb", "re", "res", "rkt", "rs", "s", "sass", "scala",
    "scm", "scss", "sh", "sol", "sql", "sv", "svelte", "svh", "swift", "tcl", "tf", "ts", "tsx", "v", "vala", "vb",
    "vhd", "vhdl", "vue", "wgsl", "zig", "zsh",
];

pub fn is_code(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if matches!(
        name,
        "Makefile"
            | "GNUmakefile"
            | "Dockerfile"
            | "CMakeLists.txt"
            | "Rakefile"
            | "BUILD"
            | "BUILD.bazel"
            | "meson.build"
            | "Justfile"
            | "justfile"
    ) {
        return true;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            let ext = ext.to_ascii_lowercase();
            CODE.binary_search(&ext.as_str()).is_ok()
        }
        _ => false,
    }
}

/// Commit authors that are programs. They inflate change counts and own no knowledge.
pub fn is_bot(name: &str, email: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let e = email.to_ascii_lowercase();
    n.contains("[bot]")
        || e.contains("[bot]")
        || n.ends_with("-bot")
        || n.ends_with(" bot")
        || [
            "dependabot",
            "renovate",
            "github-actions",
            "greenkeeper",
            "snyk-bot",
            "pre-commit-ci",
            "mergify",
            "allcontributors",
            "copybara",
            "imgbot",
            "semantic-release-bot",
            "weblate",
        ]
        .iter()
        .any(|b| n.starts_with(b))
}

/// Commit ids listed in a `.git-blame-ignore-revs` file: mass reformatting and the like.
pub fn ignore_revs(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| l.len() >= 7 && l.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|l| l.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(user: &[&str], attrs: &[(&str, &str)]) -> PathFilter {
        let user: Vec<String> = user.iter().map(|s| s.to_string()).collect();
        let attrs: Vec<(String, String)> = attrs.iter().map(|(d, p)| (d.to_string(), p.to_string())).collect();
        PathFilter::new(true, &user, &attrs).unwrap()
    }

    #[test]
    fn defaults() {
        let f = f(&[], &[]);
        assert!(f.excludes("Cargo.lock"));
        assert!(f.excludes("web/package-lock.json"));
        assert!(f.excludes("vendor/github.com/x/y.go"));
        assert!(f.excludes("a/node_modules/b/c.js"));
        assert!(f.excludes("static/app.min.js"));
        assert!(!f.excludes("src/main.rs"));
        assert!(!f.excludes("src/vendor.rs"));
        assert!(!f.excludes("distance.c"));
    }

    #[test]
    fn user_and_attributes() {
        let f = f(&["docs/", "*.svg", "/build.rs"], &[("", "src/gen/**"), ("lib", "*.pb.ts")]);
        assert!(f.excludes("docs/a/b.md"));
        assert!(f.excludes("assets/logo.svg"));
        assert!(f.excludes("build.rs"));
        assert!(!f.excludes("tools/build.rs"));
        assert!(f.excludes("src/gen/x.rs"));
        assert!(f.excludes("lib/deep/a.pb.ts"));
        assert!(!f.excludes("other/a.pb.ts"));
    }

    #[test]
    fn attributes_parse() {
        let text = "# c\n*.rs text\nsrc/gen/** linguist-generated\n*.png binary\nweb/vendor/* linguist-vendored=true\n[attr]x -diff\n";
        assert_eq!(generated_patterns(text), vec!["src/gen/**", "*.png", "web/vendor/*"]);
    }

    #[test]
    fn code() {
        assert!(CODE.windows(2).all(|w| w[0] < w[1]), "CODE must stay sorted for binary search");
        assert!(is_code("src/main.rs"));
        assert!(is_code("a/b/Component.TSX"));
        assert!(is_code("Makefile"));
        assert!(!is_code("README.md"));
        assert!(!is_code(".github/workflows/ci.yml"));
        assert!(!is_code("LICENSE"));
        assert!(!is_code(".rs"));
    }

    #[test]
    fn bots() {
        assert!(is_bot("dependabot[bot]", "49699333+dependabot[bot]@users.noreply.github.com"));
        assert!(is_bot("renovate-bot", "bot@renovateapp.com"));
        assert!(!is_bot("Abbot Smith", "abbot@example.com"));
        assert!(!is_bot("Andrew Gallant", "jamslam@gmail.com"));
    }

    #[test]
    fn ignore_revs_parse() {
        let text = "# formatting\n1234567890abcdef1234567890abcdef12345678\n\n  ABCDEF1 # short\nnot-a-rev\n";
        assert_eq!(ignore_revs(text), vec!["1234567890abcdef1234567890abcdef12345678", "abcdef1"]);
    }
}
