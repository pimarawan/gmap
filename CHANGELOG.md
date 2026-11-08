# Changelog

## 0.5.0

Rewritten from scratch. gmap now answers three questions: which files need
attention, which files secretly depend on each other, and who knows the code.

New:

- `gmap` with no command opens the map: every source file as a rectangle,
  area by lines, color by recent changes. Folders open in place.
- `hotspots`: change frequency, weighted by age, times indentation complexity.
- `coupling`: files changed in the same commits, kept only when a
  hypergeometric test says it is not chance. `--across` for pairs in
  different folders.
- `owners`: degree of authorship per file, the truck factor, and files whose
  owners have all left.
- `activity` (was `heat`): calendar, weekday by hour in the authors' own time
  zones, commits per month.
- Renames are followed. `.mailmap` and `.git-blame-ignore-revs` are read. One
  person with several emails and the same full name is one person.
- Only source code by default. Lockfiles, vendored and generated files are
  skipped, including anything `.gitattributes` marks `linguist-generated`,
  `linguist-vendored`, `-diff` or `binary`. `--all` keeps everything.
- Bots are skipped unless `--bots`.

Fixed:

- Crash on any repository whose history has a submodule ("An object with id
  ... could not be found"). git's own repository was one of them.
- The cache was written to `.gmap/` in the working tree. It now lives in
  `.git/gmap/cache` and never shows in `git status`.
- `heat --monthly` labelled its buckets "week".

Faster: history is diffed on every core. On git/git (82,725 commits), a cold
run takes 7.3 s, where 0.4 failed after 53 s and `git log --numstat` alone
takes 34 s.

Removed: the SQLite cache, `--tui` (the map is the default), the clipboard
dependency (`y` copies through the terminal with OSC 52).

## 0.4.0

- Diff accuracy: line-based diff via the `similar` crate.
- Merges are opt-in with `--include-merges`.
- Monthly heatmap buckets with `--monthly`.
- `--author` and `--author-email` filters.
- `--exclude` patterns and `.gitignore` support.
