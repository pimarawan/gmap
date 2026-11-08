use crate::analysis::{self, Analysis, Coupling, Settings, Truck};
use crate::filter::PathFilter;
use crate::history::History;
use crate::time::{self, MONTH, YEAR};
use crate::treemap::Tree;
use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Map,
    Hotspots,
    Coupling,
    People,
    Activity,
}

pub const TABS: [(Tab, &str); 5] = [
    (Tab::Map, "Map"),
    (Tab::Hotspots, "Hotspots"),
    (Tab::Coupling, "Coupling"),
    (Tab::People, "People"),
    (Tab::Activity, "Activity"),
];

/// Time windows `w` cycles through. `None` is all of history.
pub const WINDOWS: [(Option<i64>, &str); 6] = [
    (None, "all time"),
    (Some(2 * YEAR), "2 years"),
    (Some(YEAR), "year"),
    (Some(6 * MONTH), "6 months"),
    (Some(3 * MONTH), "3 months"),
    (Some(MONTH), "month"),
];

/// A scrolling list: which row is selected and which is at the top.
#[derive(Default, Clone, Copy)]
pub struct List {
    pub sel: usize,
    pub top: usize,
}

impl List {
    pub fn clamp(&mut self, len: usize, height: usize) {
        if len == 0 {
            *self = List::default();
            return;
        }
        self.sel = self.sel.min(len - 1);
        let height = height.max(1);
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + height {
            self.top = self.sel + 1 - height;
        }
        self.top = self.top.min(len.saturating_sub(height));
    }
    fn step(&mut self, d: isize, len: usize) {
        if len > 0 {
            self.sel = (self.sel as isize + d).clamp(0, len as isize - 1) as usize;
        }
    }
}

pub struct App {
    pub h: History,
    filters: [PathFilter; 2],
    pub s: Settings,
    pub a: Analysis,
    pub tree: Tree,
    pub coupling: Coupling,
    pub truck: Truck,
    pub orphans: Vec<usize>,

    pub tab: Tab,
    pub window: usize,
    /// Set by --since/--until on the command line, until `w` is pressed.
    pub custom_window: bool,
    pub help: bool,
    /// On a narrow terminal, show the details panel instead of the view.
    pub info: bool,
    pub search: Option<String>,
    pub query: String,
    pub toast: Option<(String, Instant)>,

    /// The directory the map shows, and which of its children is selected.
    pub map_root: usize,
    pub map_sel: usize,
    /// Rectangles of the map's top-level children from the last draw: (area, child index).
    pub map_hits: Vec<(Rect, usize)>,

    pub hot: List,
    pub hot_items: Vec<usize>,
    pub pairs: List,
    pub pair_items: Vec<usize>,
    pub across: bool,
    pub people: List,
    pub people_items: Vec<usize>,
    pub show_orphans: bool,
    pub orphan_list: List,
    /// Weeks to scroll back from the newest commit in the activity calendar.
    pub act_back: i64,
    /// Rows the current list had on screen, for page up and down.
    pub page: usize,
    dir_truck: HashMap<usize, Truck>,
    quit: bool,
}

impl App {
    pub(crate) fn new(h: History, filters: [PathFilter; 2], s: Settings) -> Self {
        let custom_window = s.since.is_some() || s.until.is_some();
        let a = analysis::analyze(&h, &filters[(!s.code_only) as usize], &s);
        let tree = Tree::build(&h, &a);
        let mut app = App {
            coupling: analysis::coupling(&h, &a, s.max_changeset, 3),
            truck: analysis::truck_factor(&a, &(0..a.files.len()).collect::<Vec<_>>()),
            orphans: Vec::new(),
            tree,
            a,
            h,
            filters,
            s,
            tab: Tab::Map,
            window: 0,
            custom_window,
            help: false,
            info: false,
            search: None,
            query: String::new(),
            toast: None,
            map_root: 0,
            map_sel: 0,
            map_hits: Vec::new(),
            hot: List::default(),
            hot_items: Vec::new(),
            pairs: List::default(),
            pair_items: Vec::new(),
            across: false,
            people: List::default(),
            people_items: Vec::new(),
            show_orphans: false,
            orphan_list: List::default(),
            act_back: 0,
            page: 10,
            dir_truck: HashMap::new(),
            quit: false,
        };
        app.map_root = match &app.s.prefix {
            Some(p) => app.tree.find(p),
            None => app.descend(0),
        };
        app.refresh_lists();
        app
    }

    /// Skip a root that holds a single directory and nothing else.
    fn descend(&self, mut n: usize) -> usize {
        while let [only] = self.tree.nodes[n].children[..] {
            if !self.tree.nodes[only].is_dir() {
                break;
            }
            n = only;
        }
        n
    }

    pub fn window_label(&self) -> String {
        if self.custom_window {
            return match (self.s.since, self.s.until) {
                (Some(s), Some(u)) => format!("{} to {}", time::date(s), time::date(u)),
                (Some(s), None) => format!("since {}", time::date(s)),
                (None, Some(u)) => format!("before {}", time::date(u)),
                _ => "all time".into(),
            };
        }
        match WINDOWS[self.window] {
            (None, l) => l.to_string(),
            (Some(_), l) => format!("last {l}"),
        }
    }

    fn reanalyze(&mut self) {
        let root_path = self.tree.nodes[self.map_root].path.clone();
        let sel_path = self.selected_node().map(|c| self.tree.nodes[c].path.clone());
        self.a = analysis::analyze(&self.h, &self.filters[(!self.s.code_only) as usize], &self.s);
        self.tree = Tree::build(&self.h, &self.a);
        self.coupling = analysis::coupling(&self.h, &self.a, self.s.max_changeset, 3);
        self.truck = analysis::truck_factor(&self.a, &(0..self.a.files.len()).collect::<Vec<_>>());
        self.dir_truck.clear();
        let root = self.tree.find(&root_path);
        self.map_root = if root == 0 { self.descend(0) } else { root };
        self.map_sel = sel_path
            .and_then(|p| self.tree.nodes[self.map_root].children.iter().position(|&c| self.tree.nodes[c].path == p))
            .unwrap_or(0);
        self.refresh_lists();
    }

    /// Rebuild the filtered lists after a new analysis or a new search.
    fn refresh_lists(&mut self) {
        let q = self.query.to_lowercase();
        let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
        let a = &self.a;
        let h = &self.h;
        let path = |i: usize| h.paths[a.files[i].path as usize].as_str();
        self.hot_items = a.hotspots().into_iter().filter(|&i| hit(path(i))).collect();
        let dir = |i: usize| path(i).rsplit_once('/').map_or("", |(d, _)| d);
        self.pair_items = (0..self.coupling.pairs.len())
            .filter(|&k| {
                let p = &self.coupling.pairs[k];
                (!self.across || dir(p.a) != dir(p.b)) && (hit(path(p.a)) || hit(path(p.b)))
            })
            .collect();
        let mut people: Vec<usize> = (0..a.people.len())
            .filter(|&i| {
                let p = &h.people[a.people[i].person as usize];
                hit(&p.name) || hit(&p.email)
            })
            .collect();
        people.sort_by(|&x, &y| {
            a.people[y].owns.cmp(&a.people[x].owns).then(a.people[y].commits.cmp(&a.people[x].commits))
        });
        self.people_items = people;
        let mut orphans = analysis::orphans(a, &(0..a.files.len()).collect::<Vec<_>>());
        orphans.retain(|&i| hit(path(i)));
        orphans.sort_by(|&x, &y| a.files[y].lines.cmp(&a.files[x].lines));
        self.orphans = orphans;
    }

    pub fn selected_node(&self) -> Option<usize> {
        self.tree.nodes[self.map_root].children.get(self.map_sel).copied()
    }

    /// The file the current view points at, if any.
    pub fn selected_file(&self) -> Option<usize> {
        match self.tab {
            Tab::Map => self.selected_node().and_then(|n| self.tree.nodes[n].file),
            Tab::Hotspots => self.hot_items.get(self.hot.sel).copied(),
            Tab::People if self.show_orphans => self.orphans.get(self.orphan_list.sel).copied(),
            _ => None,
        }
    }

    fn selected_path(&self) -> Option<String> {
        if let Some(f) = self.selected_file() {
            return Some(self.h.paths[self.a.files[f].path as usize].clone());
        }
        match self.tab {
            Tab::Map => self.selected_node().map(|n| self.tree.nodes[n].path.clone()),
            Tab::Coupling => self.pair_items.get(self.pairs.sel).map(|&k| {
                let p = &self.coupling.pairs[k];
                format!(
                    "{} {}",
                    self.h.paths[self.a.files[p.a].path as usize], self.h.paths[self.a.files[p.b].path as usize]
                )
            }),
            Tab::People if !self.show_orphans => self
                .people_items
                .get(self.people.sel)
                .map(|&i| self.h.people[self.a.people[i].person as usize].email.clone()),
            _ => None,
        }
    }

    /// Truck factor of a directory on the map, computed when first shown.
    pub fn dir_truck(&mut self, node: usize) -> &Truck {
        if !self.dir_truck.contains_key(&node) {
            let mut files = Vec::new();
            let mut stack = vec![node];
            while let Some(n) = stack.pop() {
                let n = &self.tree.nodes[n];
                files.extend(n.file);
                stack.extend(&n.children);
            }
            let t = analysis::truck_factor(&self.a, &files);
            self.dir_truck.insert(node, t);
        }
        &self.dir_truck[&node]
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn key(&mut self, k: KeyEvent, term: &mut ratatui::DefaultTerminal) -> Result<()> {
        if self.search.is_some() {
            self.search_key(k);
            return Ok(());
        }
        if self.help {
            self.help = false;
            return Ok(());
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('i') => self.info = !self.info,
            KeyCode::Char(c @ '1'..='5') => self.tab = TABS[c as usize - '1' as usize].0,
            KeyCode::Tab => self.tab = TABS[(self.tab_index() + 1) % TABS.len()].0,
            KeyCode::BackTab => self.tab = TABS[(self.tab_index() + TABS.len() - 1) % TABS.len()].0,
            KeyCode::Char('/') => self.search = Some(String::new()),
            KeyCode::Char('w') => self.cycle_window(1),
            KeyCode::Char('W') => self.cycle_window(WINDOWS.len() - 1),
            KeyCode::Char('a') => {
                self.s.code_only = !self.s.code_only;
                self.reanalyze();
                self.say(if self.s.code_only {
                    "source code only"
                } else {
                    "every file: docs, config and generated code too"
                });
            }
            KeyCode::Char('y') => match self.selected_path() {
                Some(p) => {
                    osc52(&p);
                    self.say(format!("copied {p}"));
                }
                None => self.say("nothing to copy here"),
            },
            KeyCode::Char('e') => match self.selected_file() {
                Some(f) => self.edit(f, term)?,
                None => self.say("select a file to open it"),
            },
            _ => match self.tab {
                Tab::Map => self.map_key(k),
                Tab::Activity => match k.code {
                    KeyCode::Left | KeyCode::Char('h') => self.act_back += 4,
                    KeyCode::Right | KeyCode::Char('l') => self.act_back = (self.act_back - 4).max(0),
                    KeyCode::Home | KeyCode::Char('g') => self.act_back = 0,
                    _ => {}
                },
                _ => self.list_key(k),
            },
        }
        Ok(())
    }

    fn search_key(&mut self, k: KeyEvent) {
        let Some(q) = &mut self.search else { return };
        match k.code {
            KeyCode::Esc => {
                self.search = None;
                self.query.clear();
            }
            KeyCode::Enter => {
                self.search = None;
                if self.tab == Tab::Map {
                    self.jump_to_match();
                }
            }
            KeyCode::Backspace => {
                q.pop();
                self.query = q.clone();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                q.push(c);
                self.query = q.clone();
            }
            _ => return,
        }
        self.refresh_lists();
        self.hot = List::default();
        self.pairs = List::default();
        self.people = List::default();
        self.orphan_list = List::default();
    }

    fn tab_index(&self) -> usize {
        TABS.iter().position(|t| t.0 == self.tab).unwrap_or(0)
    }

    fn cycle_window(&mut self, by: usize) {
        self.window = (self.window + by) % WINDOWS.len();
        self.custom_window = false;
        self.s.until = None;
        self.s.since = WINDOWS[self.window].0.map(|d| self.h.now() - d);
        self.reanalyze();
        self.say(format!("changes from the {}", self.window_label()));
    }

    fn list_key(&mut self, k: KeyEvent) {
        let (list, len) = match self.tab {
            Tab::Hotspots => (&mut self.hot, self.hot_items.len()),
            Tab::Coupling => (&mut self.pairs, self.pair_items.len()),
            Tab::People if self.show_orphans => (&mut self.orphan_list, self.orphans.len()),
            Tab::People => (&mut self.people, self.people_items.len()),
            _ => return,
        };
        let page = self.page.max(1) as isize;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => list.step(1, len),
            KeyCode::Up | KeyCode::Char('k') => list.step(-1, len),
            KeyCode::Char('d') if ctrl => list.step(page / 2, len),
            KeyCode::Char('u') if ctrl => list.step(-page / 2, len),
            KeyCode::PageDown | KeyCode::Char(' ') => list.step(page, len),
            KeyCode::PageUp => list.step(-page, len),
            KeyCode::Home | KeyCode::Char('g') => list.sel = 0,
            KeyCode::End | KeyCode::Char('G') => list.sel = len.saturating_sub(1),
            KeyCode::Char('x') if self.tab == Tab::Coupling => {
                self.across = !self.across;
                self.pairs = List::default();
                self.refresh_lists();
                self.say(if self.across { "only pairs in different directories" } else { "all pairs" });
            }
            KeyCode::Char('o') if self.tab == Tab::People => self.show_orphans = !self.show_orphans,
            KeyCode::Enter | KeyCode::Char('m') => {
                if let Some(f) = self.selected_file() {
                    self.show_on_map(f);
                }
            }
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.refresh_lists();
            }
            _ => {}
        }
    }

    /// Switch to the map with this file selected.
    fn show_on_map(&mut self, f: usize) {
        let path = self.h.paths[self.a.files[f].path as usize].clone();
        self.select_on_map(self.tree.find(&path));
        self.tab = Tab::Map;
    }

    fn select_on_map(&mut self, node: usize) {
        if let Some(parent) = self.tree.nodes[node].parent {
            self.map_root = parent;
            self.map_sel = self.tree.nodes[parent].children.iter().position(|&c| c == node).unwrap_or(0);
        }
    }

    fn jump_to_match(&mut self) {
        let q = self.query.to_lowercase();
        self.query.clear();
        if q.is_empty() {
            return;
        }
        // The biggest file whose path matches, so the jump lands somewhere that matters.
        let best = (0..self.tree.nodes.len())
            .filter(|&n| self.tree.nodes[n].file.is_some() && self.tree.nodes[n].path.to_lowercase().contains(&q))
            .max_by_key(|&n| self.tree.nodes[n].lines);
        match best {
            Some(n) => self.select_on_map(n),
            None => self.say(format!("no file matches '{q}'")),
        }
    }

    fn map_key(&mut self, k: KeyEvent) {
        let n = self.tree.nodes[self.map_root].children.len();
        match k.code {
            KeyCode::Left | KeyCode::Char('h') => self.map_move(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.map_move(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.map_move(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.map_move(0, 1),
            KeyCode::Char('n') if n > 0 => self.map_sel = (self.map_sel + 1) % n,
            KeyCode::Char('p') if n > 0 => self.map_sel = (self.map_sel + n - 1) % n,
            KeyCode::Enter => self.map_enter(),
            KeyCode::Backspace | KeyCode::Esc | KeyCode::Char('u') => self.map_up(),
            KeyCode::Home | KeyCode::Char('g') => {
                self.map_root = self.descend(0);
                self.map_sel = 0;
            }
            _ => {}
        }
    }

    fn map_enter(&mut self) {
        if let Some(c) = self.selected_node() {
            if self.tree.nodes[c].is_dir() && !self.tree.nodes[c].children.is_empty() {
                self.map_root = c;
                self.map_sel = 0;
            }
        }
    }

    fn map_up(&mut self) {
        if let Some(p) = self.tree.nodes[self.map_root].parent {
            let from = self.map_root;
            self.map_root = p;
            self.map_sel = self.tree.nodes[p].children.iter().position(|&c| c == from).unwrap_or(0);
        }
    }

    /// Move the selection to the nearest rectangle in a direction: one that
    /// lies past our edge, preferring those that overlap us sideways.
    fn map_move(&mut self, dx: i32, dy: i32) {
        let Some(&(cur, _)) = self.map_hits.iter().find(|h| h.1 == self.map_sel) else {
            self.map_sel = 0;
            return;
        };
        // Cells are about twice as tall as they are wide.
        let center = |r: Rect| (r.x as f64 + r.width as f64 / 2.0, (r.y as f64 + r.height as f64 / 2.0) * 2.0);
        let (cx, cy) = center(cur);
        let mut best: Option<(f64, usize)> = None;
        for &(r, i) in &self.map_hits {
            let ahead = match (dx, dy) {
                (1, _) => r.x >= cur.x + cur.width,
                (-1, _) => r.x + r.width <= cur.x,
                (_, 1) => r.y >= cur.y + cur.height,
                _ => r.y + r.height <= cur.y,
            };
            if i == self.map_sel || !ahead {
                continue;
            }
            let (x, y) = center(r);
            let (along, side) =
                if dx != 0 { ((x - cx).abs(), (y - cy).abs()) } else { ((y - cy).abs(), (x - cx).abs()) };
            let overlap = if dx != 0 {
                r.y < cur.y + cur.height && cur.y < r.y + r.height
            } else {
                r.x < cur.x + cur.width && cur.x < r.x + r.width
            };
            let score = along + if overlap { 0.0 } else { 3.0 * side };
            if best.is_none_or(|b| score < b.0) {
                best = Some((score, i));
            }
        }
        if let Some((_, i)) = best {
            self.map_sel = i;
        }
    }

    fn mouse(&mut self, m: event::MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if self.tab == Tab::Map => {
                let at = ratatui::layout::Position::new(m.column, m.row);
                if let Some(&(_, i)) = self.map_hits.iter().find(|(r, _)| r.contains(at)) {
                    if i == self.map_sel {
                        self.map_enter();
                    } else {
                        self.map_sel = i;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) if self.tab == Tab::Map => self.map_up(),
            MouseEventKind::ScrollDown => self.list_key(KeyEvent::from(KeyCode::Down)),
            MouseEventKind::ScrollUp => self.list_key(KeyEvent::from(KeyCode::Up)),
            _ => {}
        }
    }

    fn edit(&mut self, f: usize, term: &mut ratatui::DefaultTerminal) -> Result<()> {
        let rel = self.h.paths[self.a.files[f].path as usize].clone();
        let Some(root) = self.h.root.clone() else {
            self.say("a bare repository has no files to open");
            return Ok(());
        };
        let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
        let mut parts = editor.split_whitespace();
        let program = parts.next().unwrap_or("vi").to_string();
        let args: Vec<String> = parts.map(str::to_string).collect();
        leave();
        let status = std::process::Command::new(&program).args(&args).arg(root.join(&rel)).status();
        *term = enter();
        term.clear()?;
        if let Err(e) = status {
            self.say(format!("could not run {program}: {e}"));
        }
        Ok(())
    }
}

/// Copy text to the clipboard through the terminal (OSC 52). Works over SSH
/// and in tmux with set-clipboard on, and needs no clipboard library.
fn osc52(text: &str) {
    use std::io::Write;
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(text.len() * 4 / 3 + 4);
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, &c)| acc | (c as u32) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= chunk.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{s}\x07");
    let _ = out.flush();
}

fn enter() -> ratatui::DefaultTerminal {
    let t = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::event::EnableMouseCapture);
    t
}

fn leave() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::event::DisableMouseCapture);
    ratatui::restore();
}

/// `filters` are the default one and the one `a` switches to, which keeps everything.
pub fn run(h: History, filters: [PathFilter; 2], s: Settings) -> Result<()> {
    let mut app = App::new(h, filters, s);
    let mut term = enter();
    let result = (|| -> Result<()> {
        while !app.quit {
            if app.toast.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
                app.toast = None;
            }
            term.draw(|f| crate::ui::draw(f, &mut app))?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k, &mut term)?,
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
        }
        Ok(())
    })();
    leave();
    result
}

#[cfg(test)]
mod tests {
    use super::List;

    #[test]
    fn lists_keep_the_selection_visible() {
        let mut l = List { sel: 30, top: 0 };
        l.clamp(100, 10);
        assert_eq!(l.top, 21);
        l.sel = 5;
        l.clamp(100, 10);
        assert_eq!(l.top, 5);
        l.sel = 500;
        l.clamp(100, 10);
        assert_eq!((l.sel, l.top), (99, 90));
    }
}
