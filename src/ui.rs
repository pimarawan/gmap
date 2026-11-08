use crate::analysis::{self, FileStat};
use crate::app::{App, List, Tab, TABS};
use crate::paint::{self, darken, heat, ink, lighten};
use crate::report::{ago, resample};
use crate::time::{self, DAY, WEEK, YEAR};
use crate::treemap::{squarify, Rect as FRect};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;

const ACCENT: Color = Color::Rgb(255, 170, 60);
const TEXT: Color = Color::Rgb(220, 220, 225);
const DIM: Color = Color::Rgb(130, 130, 142);
const FAINT: Color = Color::Rgb(70, 70, 80);
const SEL: Color = Color::Rgb(44, 46, 60);
const HEADER: paint::Rgb = (32, 32, 40);
const BLUE: Color = Color::Rgb(120, 170, 255);
const GREEN: Color = Color::Rgb(90, 200, 120);
const RED: Color = Color::Rgb(235, 100, 100);

fn rgb(c: paint::Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn st(fg: Color) -> Style {
    Style::new().fg(fg)
}

fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), st(DIM))
}

fn txt(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), st(TEXT))
}

fn bold(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), st(TEXT).add_modifier(Modifier::BOLD))
}

fn head(s: &str) -> Line<'static> {
    Line::from(Span::styled(s.to_string(), st(ACCENT)))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < 60 || area.height < 16 {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(txt("gmap needs at least 60 x 16")),
            Line::from(dim("q quits")),
        ])
        .centered();
        f.render_widget(msg, area);
        return;
    }
    let [top, tabs, body, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2), Constraint::Min(5), Constraint::Length(1)])
            .areas(area);
    draw_top(f, app, top);
    draw_tabs(f, app, tabs);
    let (main, side) = if area.width >= 110 {
        let w = (area.width / 3).clamp(44, 60);
        let [m, s] = Layout::horizontal([Constraint::Min(50), Constraint::Length(w)]).areas(body);
        (Some(m), Some(s))
    } else if app.info {
        (None, Some(body))
    } else {
        (Some(body), None)
    };
    if let Some(main) = main {
        match app.tab {
            Tab::Map => draw_map(f.buffer_mut(), app, main),
            Tab::Hotspots => draw_hotspots(f, app, main),
            Tab::Coupling => draw_coupling(f, app, main),
            Tab::People => draw_people(f, app, main),
            Tab::Activity => draw_activity(f, app, main),
        }
    }
    if let Some(side) = side {
        draw_side(f, app, side);
    }
    draw_footer(f, app, foot);
    if app.help {
        draw_help(f, area);
    }
}

fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let a = &app.a;
    let kind = if app.s.code_only { "source files" } else { "files" };
    let left = Line::from(vec![
        Span::styled(" gmap ", Style::new().fg(Color::Rgb(20, 20, 24)).bg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        bold(app.h.name.clone()),
        Span::raw(" "),
        dim(app.h.branch.clone()),
        dim(format!(
            "  {} commits · {} people · {} {kind}",
            paint::thousands(a.commits.len() as u64),
            paint::thousands(a.people.len() as u64),
            paint::thousands(a.files.len() as u64)
        )),
    ]);
    f.render_widget(Paragraph::new(left), area);
    let right = Line::from(vec![Span::styled(app.window_label(), st(ACCENT)), dim("  ? help ")]);
    f.render_widget(Paragraph::new(right).right_aligned(), area);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (i, (t, name)) in TABS.iter().enumerate() {
        let on = *t == app.tab;
        spans.push(Span::styled(format!("{}", i + 1), st(if on { ACCENT } else { FAINT })));
        spans.push(Span::raw(" "));
        let style = if on { st(TEXT).add_modifier(Modifier::BOLD) } else { st(DIM) };
        spans.push(Span::styled(name.to_string(), style));
        spans.push(Span::raw("   "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), Rect { height: 1, ..area });
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(q) = &app.search {
        let what = if app.tab == Tab::Map { "jump to file" } else { "filter" };
        Line::from(vec![
            Span::styled(format!(" {what} /"), st(ACCENT)),
            txt(q.clone()),
            Span::styled("█", st(ACCENT)),
            dim("   enter done  esc clear"),
        ])
    } else if let Some((msg, _)) = &app.toast {
        Line::from(Span::styled(format!(" {msg}"), st(ACCENT)))
    } else {
        let keys: &[(&str, &str)] = match app.tab {
            Tab::Map => &[
                ("←↑↓→", "move"),
                ("enter", "open"),
                ("⌫", "up"),
                ("/", "find"),
                ("w", "window"),
                ("a", "all files"),
                ("e", "edit"),
                ("y", "copy"),
            ],
            Tab::Hotspots => &[
                ("↑↓", "move"),
                ("enter", "on map"),
                ("/", "filter"),
                ("w", "window"),
                ("a", "all files"),
                ("e", "edit"),
                ("y", "copy"),
            ],
            Tab::Coupling => {
                &[("↑↓", "move"), ("x", "across folders"), ("/", "filter"), ("w", "window"), ("y", "copy")]
            }
            Tab::People => {
                &[("↑↓", "move"), ("o", "orphaned files"), ("/", "filter"), ("w", "window"), ("y", "copy email")]
            }
            Tab::Activity => &[("←→", "scroll the year"), ("g", "back to now"), ("w", "window")],
        };
        let mut spans = vec![Span::raw(" ")];
        if area.width < 110 {
            spans.push(txt("i"));
            spans.push(dim(if app.info { " back   " } else { " details   " }));
        }
        for (k, v) in keys {
            spans.push(txt(k.to_string()));
            spans.push(dim(format!(" {v}   ")));
        }
        if !app.query.is_empty() && app.tab != Tab::Map && app.tab != Tab::Activity {
            spans.push(Span::styled(format!("/{}", app.query), st(ACCENT)));
            spans.push(dim("  esc clears"));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
    f.render_widget(Paragraph::new(Line::from(dim("q quit "))).right_aligned(), area);
}

fn draw_map(buf: &mut Buffer, app: &mut App, area: Rect) {
    let [crumbs, area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let mut x = crumbs.x + 1;
    let mut chain = vec![app.map_root];
    while let Some(p) = app.tree.nodes[*chain.last().unwrap()].parent {
        chain.push(p);
    }
    for (k, &n) in chain.iter().rev().enumerate() {
        let last = k == chain.len() - 1;
        let name = &app.tree.nodes[n].name;
        let style = if last { st(TEXT).add_modifier(Modifier::BOLD) } else { st(DIM) };
        let (nx, _) = buf.set_stringn(x, crumbs.y, name, (crumbs.right().saturating_sub(x)) as usize, style);
        x = nx;
        if !last {
            let (nx, _) = buf.set_stringn(x, crumbs.y, " / ", 3, st(FAINT));
            x = nx;
        }
    }
    let legend_w = 34u16;
    if crumbs.width > x - crumbs.x + legend_w + 2 {
        let lx = crumbs.right() - legend_w;
        buf.set_string(lx, crumbs.y, "size = lines  color = changes ", st(DIM));
        for i in 0..4u16 {
            buf.set_string(lx + 30 + i, crumbs.y, "█", st(rgb(heat(i as f64 / 3.0))));
        }
    }

    let root = app.map_root;
    let children = app.tree.nodes[root].children.clone();
    app.map_hits.clear();
    if children.is_empty() {
        buf.set_string(area.x + 1, area.y + 1, "nothing here", st(DIM));
        return;
    }
    let cells = layout(&app.tree, &children, area);
    for (i, (&c, cell)) in children.iter().zip(&cells).enumerate() {
        if cell.width == 0 || cell.height == 0 {
            continue;
        }
        app.map_hits.push((*cell, i));
        draw_node(buf, app, c, *cell, 0);
    }
    match app.map_hits.iter().find(|h| h.1 == app.map_sel) {
        Some(&(r, _)) => {
            let n = children[app.map_sel];
            if is_nested(&app.tree, n, r, 0) {
                // Light up the folder's header; a frame would cover its contents.
                let label = format!(" {}/", app.tree.nodes[n].name);
                fill(buf, Rect { height: 1, ..r }, (255, 170, 60));
                buf.set_stringn(
                    r.x,
                    r.y,
                    &label,
                    r.width as usize,
                    Style::new().fg(Color::Rgb(20, 20, 24)).add_modifier(Modifier::BOLD),
                );
            } else {
                outline(buf, r, &app.tree.nodes[n].name);
            }
        }
        None => {
            if let Some(&(_, i)) = app.map_hits.first() {
                app.map_sel = i;
            }
        }
    }
}

/// Lay out children in whole cells. Squarify works in units where a cell is
/// one wide and two tall, so squares on screen look square.
fn layout(tree: &crate::treemap::Tree, children: &[usize], area: Rect) -> Vec<Rect> {
    let sizes: Vec<f64> = children.iter().map(|&c| tree.nodes[c].lines as f64).collect();
    let rects = squarify(&sizes, FRect { x: 0.0, y: 0.0, w: area.width as f64, h: area.height as f64 * 2.0 });
    rects
        .iter()
        .map(|r| {
            // Snap both edges, so neighbours share an edge exactly and nothing overlaps.
            let x0 = r.x.round() as u16;
            let x1 = (r.x + r.w).round() as u16;
            let y0 = (r.y / 2.0).round() as u16;
            let y1 = ((r.y + r.h) / 2.0).round() as u16;
            Rect {
                x: area.x + x0.min(area.width),
                y: area.y + y0.min(area.height),
                width: x1.min(area.width).saturating_sub(x0),
                height: y1.min(area.height).saturating_sub(y0),
            }
        })
        .collect()
}

/// Whether a folder has room to show its contents under a header row.
fn is_nested(tree: &crate::treemap::Tree, n: usize, cell: Rect, depth: u8) -> bool {
    let node = &tree.nodes[n];
    // Deeper folders open only when they are big enough to show something.
    let (w, h) = match depth {
        0 | 1 => (10, 4),
        _ => (16, 6),
    };
    node.is_dir() && depth < 6 && cell.width >= w && cell.height >= h && !node.children.is_empty()
}

fn draw_node(buf: &mut Buffer, app: &App, n: usize, cell: Rect, depth: u8) {
    let node = &app.tree.nodes[n];
    let bg = heat(node.heat);
    if is_nested(&app.tree, n, cell, depth) {
        fill(buf, Rect { height: 1, ..cell }, HEADER);
        let label = format!(" {}/", node.name);
        let (x, _) =
            buf.set_stringn(cell.x, cell.y, &label, cell.width as usize, st(TEXT).add_modifier(Modifier::BOLD));
        let info = format!(" {}", paint::short(node.lines));
        if x + paint::width(&info) as u16 + 1 < cell.right() {
            buf.set_string(x, cell.y, info, st(DIM));
        }
        let inner = Rect { y: cell.y + 1, height: cell.height - 1, ..cell };
        let cells = layout(&app.tree, &node.children, inner);
        for (&c, r) in node.children.iter().zip(&cells) {
            if r.width > 0 && r.height > 0 {
                draw_node(buf, app, c, *r, depth + 1);
            }
        }
        return;
    }
    fill(buf, cell, bg);
    // A darker right and bottom edge separates neighbours without spending a cell on a border.
    let edge = darken(bg, 0.38);
    if cell.width >= 3 {
        fill(buf, Rect { x: cell.right() - 1, width: 1, ..cell }, edge);
    }
    if cell.height >= 2 {
        fill(buf, Rect { y: cell.bottom() - 1, height: 1, ..cell }, edge);
    }
    let w = cell.width.saturating_sub(1).max(1) as usize;
    let fg = rgb(ink(bg));
    let mut name = node.name.clone();
    if node.is_dir() {
        name.push('/');
    }
    let style = if node.is_dir() { Style::new().fg(fg).add_modifier(Modifier::BOLD) } else { Style::new().fg(fg) };
    if cell.width >= 3 {
        buf.set_stringn(cell.x, cell.y, paint::head(&name, w), w, style);
    }
    if cell.height >= 3 && cell.width >= 7 {
        let info = if node.is_dir() { format!("{} files", node.files) } else { paint::short(node.lines) };
        let soft = if ink(bg).0 > 128 { darken(ink(bg), 0.3) } else { lighten(ink(bg), 0.35) };
        buf.set_stringn(cell.x, cell.y + 1, &info, w, st(rgb(soft)));
    }
}

fn fill(buf: &mut Buffer, r: Rect, c: paint::Rgb) {
    let r = r.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(Style::new().bg(rgb(c)));
        }
    }
}

/// Mark the selected rectangle with a bright frame titled with its name.
fn outline(buf: &mut Buffer, r: Rect, name: &str) {
    let style = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);
    if r.width < 2 || r.height < 2 {
        for y in r.top()..r.bottom() {
            for x in r.left()..r.right() {
                buf[(x, y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
            }
        }
        return;
    }
    let (l, t, rr, b) = (r.left(), r.top(), r.right() - 1, r.bottom() - 1);
    for x in l..=rr {
        buf[(x, t)].set_symbol("━").set_style(style);
        buf[(x, b)].set_symbol("━").set_style(style);
    }
    for y in t..=b {
        buf[(l, y)].set_symbol("┃").set_style(style);
        buf[(rr, y)].set_symbol("┃").set_style(style);
    }
    buf[(l, t)].set_symbol("┏");
    buf[(rr, t)].set_symbol("┓");
    buf[(l, b)].set_symbol("┗");
    buf[(rr, b)].set_symbol("┛");
    if r.width > 6 {
        let title = format!(" {} ", paint::head(name, r.width as usize - 4));
        buf.set_string(l + 1, t, title, style);
    }
}

/// Rows for a scrolled list: the visible slice, and the row height left after the header.
fn visible(list: &mut List, len: usize, area: Rect, header: u16) -> (std::ops::Range<usize>, usize) {
    let height = area.height.saturating_sub(header) as usize;
    list.clamp(len, height);
    (list.top..(list.top + height).min(len), height)
}

fn row_style(selected: bool) -> Style {
    if selected {
        Style::new().bg(SEL)
    } else {
        Style::new()
    }
}

fn owner_span(app: &App, f: &FileStat, w: usize) -> Span<'static> {
    match f.owners.first() {
        Some(&p) => {
            let gone = app.a.person(p).is_some_and(|p| !p.active);
            let name = &app.h.people[p as usize].name;
            if gone {
                Span::styled(paint::fit(&format!("{}†", paint::head(name, w - 1)), w), st(DIM))
            } else {
                txt(paint::fit(name, w))
            }
        }
        None => Span::styled(paint::fit("nobody", w), st(RED)),
    }
}

fn draw_hotspots(f: &mut Frame, app: &mut App, area: Rect) {
    let len = app.hot_items.len();
    let mut list = app.hot;
    let (rows, height) = visible(&mut list, len, area, 2);
    app.hot = list;
    app.page = height;
    let mut lines = vec![
        Line::from(dim("    #  score     changes  last 12 months   lines  owner             file")),
        Line::from(""),
    ];
    if len == 0 {
        lines.push(Line::from(dim("    no file changed in this window")));
    }
    let max = app.hot_items.first().map_or(1.0, |&i| app.a.files[i].score.max(1e-9));
    let a = &app.a;
    for k in rows {
        let i = app.hot_items[k];
        let file = &a.files[i];
        let t = file.score / max;
        let trend = analysis::histogram(&app.h, &file.commits, a.now - YEAR, a.now, 12);
        let sel = k == app.hot.sel;
        let path = &app.h.paths[file.path as usize];
        let used = 69;
        let path_w = (area.width as usize).saturating_sub(used).max(8);
        let (dir, name) = split_path(&paint::tail(path, path_w));
        let line = Line::from(vec![
            dim(format!("{:>5}  ", k + 1)),
            Span::styled(paint::bar(t, 8), st(rgb(heat(t)))),
            txt(format!("  {:>6}  ", paint::thousands(file.revs as u64))),
            Span::styled(format!("{:<14}", paint::spark(&trend)), st(BLUE)),
            txt(format!("  {:>6}  ", paint::short(file.lines as u64))),
            owner_span(app, file, 16),
            Span::raw("  "),
            dim(dir),
            if sel { bold(name) } else { txt(name) },
        ])
        .style(row_style(sel));
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn split_path(p: &str) -> (String, String) {
    match p.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), p.to_string()),
    }
}

fn draw_coupling(f: &mut Frame, app: &mut App, area: Rect) {
    let len = app.pair_items.len();
    let mut list = app.pairs;
    let (rows, height) = visible(&mut list, len, area, 2);
    app.pairs = list;
    app.page = height;
    let title = if app.across {
        "   together  shared  files that change together, in different folders"
    } else {
        "   together  shared  files that change together"
    };
    let mut lines = vec![Line::from(dim(title)), Line::from("")];
    if len == 0 {
        lines.push(Line::from(dim("   no pair changes together more often than chance explains")));
    }
    let a = &app.a;
    for k in rows {
        let p = &app.coupling.pairs[app.pair_items[k]];
        let pa = &app.h.paths[a.files[p.a].path as usize];
        let pb = &app.h.paths[a.files[p.b].path as usize];
        let (common, ra, rb) = common_dir(pa, pb);
        let sel = k == app.pairs.sel;
        let room = (area.width as usize).saturating_sub(24 + paint::width(&common));
        let half = (room / 2).max(8);
        let across = pa.rsplit_once('/').map(|x| x.0) != pb.rsplit_once('/').map(|x| x.0);
        let line = Line::from(vec![
            Span::styled(format!("   {:>3.0}% {}", p.degree * 100.0, paint::bar(p.degree, 3)), st(rgb(heat(p.degree)))),
            txt(format!("  {:>5}  ", p.shared)),
            dim(common),
            if sel { bold(paint::tail(&ra, half)) } else { txt(paint::tail(&ra, half)) },
            Span::styled(" ↔ ", st(if across { ACCENT } else { FAINT })),
            if sel { bold(paint::tail(&rb, half)) } else { txt(paint::tail(&rb, half)) },
        ])
        .style(row_style(sel));
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The shared leading directories of two paths, and what is left of each.
fn common_dir(a: &str, b: &str) -> (String, String, String) {
    let mut end = 0;
    for (i, (x, y)) in a.bytes().zip(b.bytes()).enumerate() {
        if x != y {
            break;
        }
        if x == b'/' {
            end = i + 1;
        }
    }
    (a[..end].to_string(), a[end..].to_string(), b[end..].to_string())
}

fn draw_people(f: &mut Frame, app: &mut App, area: Rect) {
    let t = &app.truck;
    let names: Vec<&str> = t.key.iter().map(|&p| app.h.people[p as usize].name.as_str()).collect();
    let factor_color = rgb(paint::truck_color(t.factor));
    let mut lines = vec![Line::from(vec![
        txt("  truck factor "),
        Span::styled(t.factor.to_string(), Style::new().fg(factor_color).add_modifier(Modifier::BOLD)),
        dim(match t.factor {
            0 => "   most files already have nobody who knows them well".to_string(),
            1 => format!("   if {} left, most files would have nobody who knows them", names[0]),
            _ => format!("   if these {} left, most files would have nobody who knows them", t.factor),
        }),
    ])];
    let mut curve = vec![dim("  files still known as people leave  ")];
    for (k, c) in t.coverage.iter().enumerate() {
        if k > 0 {
            curve.push(Span::styled(" → ", st(FAINT)));
        }
        curve.push(Span::styled(format!("{:.0}%", c * 100.0), st(if *c < 0.5 { RED } else { TEXT })));
    }
    lines.push(Line::from(curve));
    lines.push(Line::from(""));

    let body = Rect { y: area.y + 3, height: area.height.saturating_sub(3), ..area };
    if app.show_orphans {
        let len = app.orphans.len();
        let mut list = app.orphan_list;
        let (rows, height) = visible(&mut list, len, body, 2);
        app.orphan_list = list;
        app.page = height;
        lines.push(Line::from(vec![
            Span::styled(format!("  {} files ", paint::thousands(len as u64)), st(ACCENT)),
            dim(format!("are known only by people with no commit in the last {}", time::span(app.s.inactive))),
        ]));
        lines.push(Line::from(dim("    lines  known by               file")));
        for k in rows {
            let i = app.orphans[k];
            let file = &app.a.files[i];
            let who: Vec<&str> = file.owners.iter().map(|&p| app.h.people[p as usize].name.as_str()).collect();
            let sel = k == app.orphan_list.sel;
            let path = &app.h.paths[file.path as usize];
            let w = (area.width as usize).saturating_sub(36).max(8);
            let (dir, name) = split_path(&paint::tail(path, w));
            lines.push(
                Line::from(vec![
                    txt(format!("  {:>7}  ", paint::thousands(file.lines as u64))),
                    dim(paint::fit(&who.join(", "), 21)),
                    Span::raw("  "),
                    dim(dir),
                    if sel { bold(name) } else { txt(name) },
                ])
                .style(row_style(sel)),
            );
        }
    } else {
        let len = app.people_items.len();
        let mut list = app.people;
        let (rows, height) = visible(&mut list, len, body, 1);
        app.people = list;
        app.page = height;
        lines.push(Line::from(dim("     owns  commits     added   deleted  last commit      person")));
        for k in rows {
            let p = &app.a.people[app.people_items[k]];
            let who = &app.h.people[p.person as usize];
            let sel = k == app.people.sel;
            let key = app.truck.key.contains(&p.person);
            let name_style = match (sel, p.active) {
                (_, false) => st(DIM),
                (true, true) => st(TEXT).add_modifier(Modifier::BOLD),
                _ => st(TEXT),
            };
            let mut spans = vec![
                txt(format!("  {:>7}  {:>7}  ", paint::thousands(p.owns as u64), paint::thousands(p.commits as u64))),
                Span::styled(format!("{:>8}", format!("+{}", paint::short(p.added))), st(GREEN)),
                Span::styled(format!("{:>10}", format!("-{}", paint::short(p.deleted))), st(RED)),
                dim(format!("  {:<15}  ", ago(app.a.now, p.last))),
                Span::styled(who.name.clone(), name_style),
            ];
            if key {
                spans.push(Span::styled("  key", st(ACCENT)));
            }
            if !p.active {
                spans.push(dim("  gone"));
            }
            lines.push(Line::from(spans).style(row_style(sel)));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_activity(f: &mut Frame, app: &App, area: Rect) {
    let act = analysis::activity(&app.h, &app.a.commits);
    if act.days.is_empty() {
        f.render_widget(Paragraph::new(Line::from(dim("  no commits in this window"))), area);
        return;
    }
    let buf = f.buffer_mut();
    let days: HashMap<i64, u32> = act.days.iter().copied().collect();
    let x0 = area.x + 2;
    let mut y = area.y;

    // The calendar: one column per week, Monday at the top.
    let end = act.days.last().unwrap().0 - app.act_back * 7;
    let weeks = (((area.width.saturating_sub(8)) / 2) as i64).clamp(4, 53);
    let start = end - time::weekday(end) as i64 - (weeks - 1) * 7;
    let mut nz: Vec<u32> = (0..weeks * 7).filter_map(|k| days.get(&(start + k)).copied()).collect();
    nz.sort_unstable();
    let q = |f: f64| nz.get(((nz.len().max(1) - 1) as f64 * f).round() as usize).copied().unwrap_or(1);
    let (q1, q2, q3) = (q(0.25), q(0.5), q(0.75));
    let mut last_month = 0;
    let mut free = 0u16;
    for w in 0..weeks {
        let (yy, m, _) = time::civil_from_days(start + w * 7);
        let x = x0 + 4 + w as u16 * 2;
        if m != last_month && w < weeks - 2 && x >= free {
            let label = if m == 1 || w == 0 {
                format!("{} {yy}", time::MONTHS[m as usize - 1])
            } else {
                time::MONTHS[m as usize - 1].to_string()
            };
            let (end, _) =
                buf.set_stringn(x, y, &label, ((weeks - w) * 2) as usize, st(if m == 1 { TEXT } else { DIM }));
            free = end + 1;
        }
        last_month = m;
    }
    y += 1;
    for wd in 0..7 {
        buf.set_string(x0, y + wd as u16, ["Mon", "", "Wed", "", "Fri", "", "Sun"][wd], st(FAINT));
        for w in 0..weeks {
            let d = start + w * 7 + wd as i64;
            if d > end {
                break;
            }
            let n = days.get(&d).copied().unwrap_or(0);
            let lvl = match n {
                0 => 0,
                n if n <= q1 => 1,
                n if n <= q2 => 2,
                n if n <= q3 => 3,
                _ => 4,
            };
            buf.set_string(x0 + 4 + w as u16 * 2, y + wd as u16, "■", st(rgb(paint::GREENS[lvl])));
        }
    }
    y += 8;
    let in_view: u32 = (0..weeks * 7).filter_map(|k| days.get(&(start + k))).sum();
    buf.set_string(
        x0,
        y,
        format!(
            "{} commits from {} to {}",
            paint::thousands(in_view as u64),
            time::date(start * DAY),
            time::date(end * DAY)
        ),
        st(DIM),
    );
    y += 2;
    if y + 9 > area.bottom() {
        return;
    }

    // Weekday by hour, in each author's own time zone.
    let max = act.punch.iter().flatten().copied().max().unwrap_or(1).max(1);
    buf.set_string(x0 + 4, y, "0     3     6     9     12    15    18    21", st(FAINT));
    buf.set_string(x0 + 54, y, "hour of day, author's time", st(DIM));
    y += 1;
    for (wd, row) in act.punch.iter().enumerate() {
        buf.set_string(x0, y + wd as u16, ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][wd], st(FAINT));
        for (hr, &n) in row.iter().enumerate() {
            let t = n as f64 / max as f64;
            buf.set_string(x0 + 4 + hr as u16 * 2, y + wd as u16, paint::dot(t), st(rgb(heat(0.2 + 0.8 * t))));
        }
    }
    y += 8;
    if y + 3 > area.bottom() {
        return;
    }

    // Every month of the window, as tall as there is room for.
    let (m0, m1) = (month_key(act.first), month_key(act.last));
    let mut series = vec![0u32; (m1 - m0 + 1) as usize];
    for &(d, n) in &act.days {
        series[(month_key(d * DAY) - m0) as usize] += n;
    }
    let w = area.width.saturating_sub(6) as usize;
    // Stretch a short history so each month is several columns wide.
    let series: Vec<u32> = if series.len() * 2 <= w {
        let k = w / series.len();
        series.iter().flat_map(|&v| std::iter::repeat_n(v, k)).collect()
    } else {
        resample(&series, w)
    };
    let rows = (area.bottom().saturating_sub(y + 3) as usize).clamp(1, 4);
    for (r, line) in paint::columns(&series, rows).into_iter().enumerate() {
        buf.set_string(x0, y + r as u16, line, st(rgb(paint::GREENS[3])));
    }
    y += rows as u16;
    let left = time::month(act.first);
    let right = time::month(act.last);
    buf.set_string(x0, y, &left, st(DIM));
    let rx = x0 + series.len().saturating_sub(right.len()) as u16;
    buf.set_string(rx.max(x0 + left.len() as u16 + 1), y, &right, st(DIM));
    buf.set_string(x0 + left.len() as u16 + 2, y, "commits per month", st(FAINT));
    y += 2;
    if y < area.bottom() {
        let (streak, streak_end) = crate::report::longest_streak(&act.days);
        let busiest = act.days.iter().max_by_key(|d| (d.1, -d.0)).unwrap();
        let line = Line::from(vec![
            txt(paint::thousands(act.days.len() as u64)),
            dim(" days with commits   busiest "),
            txt(time::date(busiest.0 * DAY)),
            dim(format!(" with {}   longest streak ", busiest.1)),
            txt(format!("{streak} days")),
            dim(if streak > 1 { format!(", to {}", time::date(streak_end * DAY)) } else { String::new() }),
        ]);
        buf.set_line(x0, y, &line, area.width.saturating_sub(4));
    }
}

fn month_key(t: i64) -> i64 {
    let (y, m, _) = time::civil_from_days(t.div_euclid(DAY));
    y * 12 + m as i64 - 1
}

fn draw_side(f: &mut Frame, app: &mut App, area: Rect) {
    for y in area.top()..area.bottom() {
        f.buffer_mut()[(area.x, y)].set_symbol("│").set_style(st(FAINT));
    }
    let inner = Rect { x: area.x + 2, width: area.width.saturating_sub(3), ..area };
    let w = inner.width as usize;
    let lines = match app.tab {
        Tab::Map => match app.selected_node() {
            Some(n) if app.tree.nodes[n].file.is_some() => file_panel(app, app.tree.nodes[n].file.unwrap(), w),
            Some(n) => dir_panel(app, n, w),
            None => vec![],
        },
        Tab::Hotspots => app.hot_items.get(app.hot.sel).map_or(vec![], |&i| file_panel(app, i, w)),
        Tab::Coupling => app.pair_items.get(app.pairs.sel).map_or(vec![], |&k| pair_panel(app, k, w)),
        Tab::People if app.show_orphans => {
            app.orphans.get(app.orphan_list.sel).map_or(vec![], |&i| file_panel(app, i, w))
        }
        Tab::People => app.people_items.get(app.people.sel).map_or(vec![], |&i| person_panel(app, i, w)),
        Tab::Activity => activity_panel(app, w),
    };
    f.render_widget(Paragraph::new(lines), inner);
}

fn path_title(path: &str, w: usize) -> Vec<Line<'static>> {
    let (dir, name) = split_path(path);
    let mut v = Vec::new();
    if !dir.is_empty() {
        v.push(Line::from(dim(paint::tail(&dir, w))));
    }
    v.push(Line::from(Span::styled(paint::head(&name, w), st(TEXT).add_modifier(Modifier::BOLD))));
    v
}

fn person_name(app: &App, p: u32) -> Span<'static> {
    let name = app.h.people[p as usize].name.clone();
    match app.a.person(p) {
        Some(s) if !s.active => Span::styled(format!("{name}  (left {} ago)", time::span(app.a.now - s.last)), st(DIM)),
        _ => txt(name),
    }
}

fn file_panel(app: &App, i: usize, w: usize) -> Vec<Line<'static>> {
    let a = &app.a;
    let file = &a.files[i];
    let mut v = path_title(&app.h.paths[file.path as usize], w);
    v.push(Line::from(vec![
        txt(paint::thousands(file.lines as u64)),
        dim(" lines   complexity "),
        txt(paint::thousands(file.complexity as u64)),
    ]));
    if file.revs == 0 {
        v.push(Line::from(dim("no changes in this window")));
    } else {
        v.push(Line::from(vec![
            txt(paint::thousands(file.revs as u64)),
            dim(if file.revs == 1 { " change   " } else { " changes   " }),
            Span::styled(format!("+{}", paint::short(file.added)), st(GREEN)),
            Span::raw(" "),
            Span::styled(format!("-{}", paint::short(file.deleted)), st(RED)),
        ]));
        v.push(Line::from(dim(format!("first {}   last {}", time::date(file.first), ago(a.now, file.last)))));
    }
    if let Some(c) = file.creator {
        v.push(Line::from(vec![dim("created by "), person_name(app, c)]));
    }

    if file.revs > 0 {
        v.push(Line::from(""));
        v.push(head("changes over time"));
        let from = file.first.min(a.now - YEAR);
        let series = analysis::histogram(&app.h, &file.commits, from, a.now, w.min(48));
        v.push(Line::from(Span::styled(paint::spark(&series), st(BLUE))));
        let (l, r) = (time::month(from), time::month(a.now));
        v.push(Line::from(dim(format!("{l}{}{r}", " ".repeat(series.len().saturating_sub(l.len() + r.len()))))));
    }

    v.push(Line::from(""));
    v.push(head("who knows it"));
    if file.owners.is_empty() {
        v.push(Line::from(Span::styled("nobody: no one has enough history with it", st(RED))));
    }
    for &p in file.owners.iter().take(3) {
        v.push(Line::from(person_name(app, p)));
    }
    if file.owners.len() > 1 || file.authors.len() > 1 {
        v.push(Line::from(""));
        v.push(head("commits by person"));
        let max = file.authors.first().map_or(1, |x| x.1).max(1);
        for &(p, n) in file.authors.iter().take(5) {
            let gone = a.person(p).is_some_and(|s| !s.active);
            v.push(Line::from(vec![
                Span::styled(paint::bar(n as f64 / max as f64, 8), st(if gone { FAINT } else { BLUE })),
                txt(format!(" {:>4}  ", n)),
                Span::styled(
                    paint::head(&app.h.people[p as usize].name, w.saturating_sub(15)),
                    st(if gone { DIM } else { TEXT }),
                ),
            ]));
        }
    }

    let mut partners: Vec<(f64, usize)> = app
        .coupling
        .pairs
        .iter()
        .filter_map(|p| {
            if p.a == i {
                Some((p.degree, p.b))
            } else if p.b == i {
                Some((p.degree, p.a))
            } else {
                None
            }
        })
        .collect();
    partners.sort_by(|x, y| y.0.total_cmp(&x.0));
    if !partners.is_empty() {
        v.push(Line::from(""));
        v.push(head("changes together with"));
        for (d, j) in partners.into_iter().take(4) {
            let other = &app.h.paths[a.files[j].path as usize];
            let (_, rest_a, rest_b) = common_dir(&app.h.paths[file.path as usize], other);
            let _ = rest_a;
            v.push(Line::from(vec![
                Span::styled(format!("{:>3.0}%  ", d * 100.0), st(rgb(heat(d)))),
                txt(paint::tail(&rest_b, w.saturating_sub(6))),
            ]));
        }
    }

    if !file.commits.is_empty() {
        v.push(Line::from(""));
        v.push(head("latest changes"));
        for &ci in file.commits.iter().take(5) {
            v.push(commit_line(app, ci, w));
        }
    }
    v
}

fn commit_line(app: &App, ci: u32, w: usize) -> Line<'static> {
    let c = &app.h.commits[ci as usize];
    let who = &app.h.people[c.author as usize].name;
    let first = who.split_whitespace().next().unwrap_or(who);
    let left = format!("{}  {}  ", time::date(c.time), paint::head(first, 10));
    let room = w.saturating_sub(paint::width(&left));
    Line::from(vec![dim(left), txt(paint::head(&c.summary, room))])
}

fn dir_panel(app: &mut App, n: usize, w: usize) -> Vec<Line<'static>> {
    let truck = app.dir_truck(n);
    let (factor, key) = (truck.factor, truck.key.clone());
    let app = &*app;
    let node = &app.tree.nodes[n];
    let mut v = path_title(&format!("{}/", node.path), w);
    v.push(Line::from(vec![
        txt(paint::thousands(node.files as u64)),
        dim(" files   "),
        txt(paint::thousands(node.lines)),
        dim(" lines"),
    ]));
    v.push(Line::from(vec![txt(paint::thousands(node.revs)), dim(" file changes in this window")]));
    v.push(Line::from(""));
    v.push(Line::from(vec![
        dim("truck factor "),
        Span::styled(factor.to_string(), Style::new().fg(rgb(paint::truck_color(factor))).add_modifier(Modifier::BOLD)),
    ]));
    for &p in key.iter().take(4) {
        v.push(Line::from(vec![dim("  "), person_name(app, p)]));
    }

    let mut files = Vec::new();
    let mut stack = vec![n];
    while let Some(k) = stack.pop() {
        let nd = &app.tree.nodes[k];
        files.extend(nd.file);
        stack.extend(&nd.children);
    }
    files.sort_by(|&x, &y| app.a.files[y].score.total_cmp(&app.a.files[x].score));
    let max = files.first().map_or(1.0, |&i| app.a.files[i].score.max(1e-9));
    if files.first().is_some_and(|&i| app.a.files[i].revs > 0) {
        v.push(Line::from(""));
        v.push(head("hottest files"));
        for &i in files.iter().take(6).filter(|&&i| app.a.files[i].revs > 0) {
            let t = app.a.files[i].score / max;
            let rel = app.h.paths[app.a.files[i].path as usize]
                .strip_prefix(&format!("{}/", node.path))
                .unwrap_or("")
                .to_string();
            v.push(Line::from(vec![
                Span::styled(paint::bar(t, 6), st(rgb(heat(t)))),
                Span::raw("  "),
                txt(paint::tail(&rel, w.saturating_sub(8))),
            ]));
        }
    }

    let mut owners: HashMap<u32, u32> = HashMap::new();
    for &i in &files {
        for &p in &app.a.files[i].owners {
            *owners.entry(p).or_default() += 1;
        }
    }
    let mut owners: Vec<(u32, u32)> = owners.into_iter().collect();
    owners.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    if !owners.is_empty() {
        v.push(Line::from(""));
        v.push(head("who knows it"));
        for (p, k) in owners.into_iter().take(5) {
            v.push(Line::from(vec![txt(format!("{:>4} ", k)), dim("files  "), person_name(app, p)]));
        }
    }
    v
}

fn pair_panel(app: &App, k: usize, w: usize) -> Vec<Line<'static>> {
    let a = &app.a;
    let p = &app.coupling.pairs[k];
    let (fa, fb) = (&a.files[p.a], &a.files[p.b]);
    let mut v = path_title(&app.h.paths[fa.path as usize], w);
    v.push(Line::from(Span::styled("↔", st(ACCENT))));
    v.extend(path_title(&app.h.paths[fb.path as usize], w));
    v.push(Line::from(""));
    v.push(Line::from(vec![txt(format!("{} ", p.shared)), dim("commits changed both")]));
    v.push(Line::from(dim(format!("first changed in {}, second in {}", p.revs_a, p.revs_b))));
    v.push(Line::from(vec![
        dim("together "),
        Span::styled(format!("{:.0}%", p.degree * 100.0), st(rgb(heat(p.degree)))),
    ]));
    v.push(Line::from(vec![dim("chance of this by accident: "), txt(odds(p.p))]));
    v.push(Line::from(""));
    v.push(head("commits that changed both"));
    // Both lists ascend by commit index, so a merge walk finds the shared ones.
    let (mut i, mut j) = (0, 0);
    let mut n = 0;
    while i < fa.commits.len() && j < fb.commits.len() && n < 12 {
        match fa.commits[i].cmp(&fb.commits[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                v.push(commit_line(app, fa.commits[i], w));
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    v
}

/// "1 in 3.2 million"
fn odds(p: f64) -> String {
    if p <= 0.0 || !p.is_finite() {
        return "less than 1 in a trillion".into();
    }
    let n = 1.0 / p;
    match n {
        n if n < 1e3 => format!("1 in {:.0}", n),
        n if n < 1e6 => format!("1 in {}", paint::thousands(n.round() as u64)),
        n if n < 1e9 => format!("1 in {:.1} million", n / 1e6),
        n if n < 1e12 => format!("1 in {:.1} billion", n / 1e9),
        _ => "less than 1 in a trillion".into(),
    }
}

fn person_panel(app: &App, i: usize, w: usize) -> Vec<Line<'static>> {
    let a = &app.a;
    let p = &a.people[i];
    let who = &app.h.people[p.person as usize];
    let mut v = vec![
        Line::from(Span::styled(paint::head(&who.name, w), st(TEXT).add_modifier(Modifier::BOLD))),
        Line::from(dim(paint::head(&who.email, w))),
        Line::from(""),
        Line::from(vec![
            txt(paint::thousands(p.commits as u64)),
            dim(" commits   "),
            Span::styled(format!("+{}", paint::short(p.added)), st(GREEN)),
            Span::raw(" "),
            Span::styled(format!("-{}", paint::short(p.deleted)), st(RED)),
        ]),
        Line::from(dim(format!("first {}   last {}", time::date(p.first), ago(a.now, p.last)))),
        Line::from(if p.active {
            Span::styled("active", st(GREEN))
        } else {
            Span::styled(format!("no commit in {}", time::span(a.now - p.last)), st(DIM))
        }),
    ];
    if app.truck.key.contains(&p.person) {
        v.push(Line::from(Span::styled("one of the people the truck factor counts", st(ACCENT))));
    }

    let mine: Vec<u32> = a.commits.iter().copied().filter(|&c| app.h.commits[c as usize].author == p.person).collect();
    if !mine.is_empty() {
        let first = app.h.commits[*mine.last().unwrap() as usize].time;
        v.push(Line::from(""));
        v.push(head("commits over time"));
        let series = analysis::histogram(&app.h, &mine, first, a.now, w.min(48));
        v.push(Line::from(Span::styled(paint::spark(&series), st(GREEN))));
        let (l, r) = (time::month(first), time::month(a.now));
        v.push(Line::from(dim(format!("{l}{}{r}", " ".repeat(series.len().saturating_sub(l.len() + r.len()))))));
    }

    let mut owned: Vec<usize> = (0..a.files.len()).filter(|&f| a.files[f].owners.contains(&p.person)).collect();
    owned.sort_by(|&x, &y| a.files[y].lines.cmp(&a.files[x].lines));
    v.push(Line::from(""));
    v.push(head(&format!("knows {} files", owned.len())));
    for &f in owned.iter().take(10) {
        let file = &a.files[f];
        let alone = file.owners.len() == 1;
        v.push(Line::from(vec![
            txt(format!("{:>6}  ", paint::short(file.lines as u64))),
            Span::styled(
                paint::tail(&app.h.paths[file.path as usize], w.saturating_sub(10)),
                st(if alone { TEXT } else { DIM }),
            ),
        ]));
    }
    if owned.len() > 10 {
        v.push(Line::from(dim(format!("and {} more", owned.len() - 10))));
    }
    v
}

fn activity_panel(app: &App, w: usize) -> Vec<Line<'static>> {
    let a = &app.a;
    let mut v = vec![head("most commits"), Line::from("")];
    let max = a.people.first().map_or(1, |p| p.commits).max(1);
    for p in a.people.iter().take(12) {
        v.push(Line::from(vec![
            Span::styled(paint::bar(p.commits as f64 / max as f64, 8), st(if p.active { GREEN } else { FAINT })),
            txt(format!(" {:>6}  ", paint::thousands(p.commits as u64))),
            Span::styled(
                paint::head(&app.h.people[p.person as usize].name, w.saturating_sub(17)),
                st(if p.active { TEXT } else { DIM }),
            ),
        ]));
    }
    let active = a.people.iter().filter(|p| p.active).count();
    v.push(Line::from(""));
    v.push(Line::from(vec![
        txt(paint::thousands(active as u64)),
        dim(format!(
            " of {} people committed in the last {}",
            paint::thousands(a.people.len() as u64),
            time::span(app.s.inactive)
        )),
    ]));
    let recent = a.commits.iter().filter(|&&c| app.h.commits[c as usize].time > a.now - 4 * WEEK).count();
    v.push(Line::from(vec![txt(paint::thousands(recent as u64)), dim(" commits in the last 4 weeks")]));
    v
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 78.min(area.width - 4);
    let h = 30.min(area.height - 2);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    let block = ratatui::widgets::Block::bordered().border_style(st(FAINT)).title(Span::styled(" gmap ", st(ACCENT)));
    let k =
        |key: &str, what: &str| Line::from(vec![Span::styled(format!("  {key:<14}"), st(TEXT)), dim(what.to_string())]);
    let lines = vec![
        Line::from(""),
        Line::from(vec![bold("  Map       "), dim("every source file; area is lines, color is recent changes")]),
        Line::from(vec![bold("  Hotspots  "), dim("complicated files that keep changing: where bugs collect")]),
        Line::from(vec![bold("  Coupling  "), dim("files that change together more than chance explains")]),
        Line::from(vec![bold("  People    "), dim("who knows each file, and the truck factor")]),
        Line::from(vec![bold("  Activity  "), dim("when the commits happen")]),
        Line::from(""),
        k("1-5  tab", "switch view"),
        k("arrows  hjkl", "move; on the map, to the next rectangle that way"),
        k("enter", "open a folder; in lists, show the file on the map"),
        k("backspace esc", "up a folder"),
        k("/", "filter lists; on the map, jump to a file"),
        k("w  W", "time window: all, 2y, 1y, 6m, 3m, 1m"),
        k("a", "source code only, or every file"),
        k("x", "coupling: only pairs in different folders"),
        k("o", "people: files nobody active knows"),
        k("e", "open the file in $VISUAL or $EDITOR"),
        k("y", "copy the path"),
        k("i", "details, when the terminal is too narrow to show them beside"),
        k("q", "quit"),
        Line::from(""),
        Line::from(dim("  A change counts half as much for every year of age (--half-life).")),
        Line::from(dim("  Ownership is degree of authorship (Fritz et al. 2014); the truck factor")),
        Line::from(dim("  removes top owners until half the files have none (Avelino et al. 2016).")),
        Line::from(dim("  Coupling pairs pass a hypergeometric test, Bonferroni corrected.")),
        Line::from(""),
        Line::from(dim("  any key closes this")),
    ];
    f.render_widget(Paragraph::new(lines).block(block), r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_dirs() {
        assert_eq!(common_dir("a/b/c.rs", "a/b/d.rs"), ("a/b/".into(), "c.rs".into(), "d.rs".into()));
        assert_eq!(common_dir("a/bc.rs", "a/bd.rs"), ("a/".into(), "bc.rs".into(), "bd.rs".into()));
        assert_eq!(common_dir("x.rs", "y.rs"), ("".into(), "x.rs".into(), "y.rs".into()));
    }

    #[test]
    fn odds_read_well() {
        assert_eq!(odds(0.01), "1 in 100");
        assert_eq!(odds(1.0 / 3_200_000.0), "1 in 3.2 million");
        assert_eq!(odds(0.0), "less than 1 in a trillion");
    }

    #[test]
    fn layout_tiles_the_area() {
        use crate::treemap::{Node, Tree};
        let mk = |lines| Node {
            name: String::new(),
            path: String::new(),
            parent: None,
            children: vec![],
            file: Some(0),
            lines,
            files: 1,
            weight: 0.0,
            revs: 0,
            heat: 0.0,
        };
        let tree = Tree { nodes: vec![mk(50), mk(30), mk(10), mk(7), mk(3)] };
        let area = Rect { x: 3, y: 2, width: 40, height: 12 };
        let cells = layout(&tree, &[0, 1, 2, 3, 4], area);
        let total: u32 = cells.iter().map(|c| c.width as u32 * c.height as u32).sum();
        assert_eq!(total, 40 * 12);
        for (i, a) in cells.iter().enumerate() {
            assert!(area.contains(a.as_position()) || a.area() == 0);
            for b in &cells[i + 1..] {
                assert!(a.intersection(*b).area() == 0);
            }
        }
    }
}

/// Renders every view of a real repository to HTML, colors included, so the
/// design can be looked at without a terminal:
/// `GMAP_SNAP=../some-repo cargo test snapshots -- --ignored`
#[cfg(test)]
mod snapshots {
    use super::*;
    use crate::app::{App, Tab};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn color(c: Color, default: &str) -> String {
        match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::White => "#ffffff".into(),
            _ => default.into(),
        }
    }

    /// The buffer as an SVG: one rectangle per run of background, one text
    /// element per run of style, each stretched to its exact cell width so the
    /// grid holds whatever monospace font the viewer has.
    fn svg(buf: &Buffer) -> String {
        const W: f64 = 8.4;
        const H: f64 = 18.0;
        const BG: &str = "#16161c";
        let (cols, rows) = (buf.area.width, buf.area.height);
        let mut s = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {:.1} {:.1}\" width=\"{:.0}\" height=\"{:.0}\">
<rect width=\"100%\" height=\"100%\" rx=\"6\" fill=\"{BG}\"/>
<g transform=\"translate(10 10)\" font-family=\"ui-monospace,SFMono-Regular,Menlo,Consolas,'Cascadia Mono',monospace\" font-size=\"14\">
",
            cols as f64 * W + 20.0,
            rows as f64 * H + 20.0,
            cols as f64 * W + 20.0,
            rows as f64 * H + 20.0
        );
        let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        for y in 0..rows {
            let mut x = 0;
            while x < cols {
                let bg = color(buf[(x, y)].bg, BG);
                let start = x;
                while x < cols && color(buf[(x, y)].bg, BG) == bg {
                    x += 1;
                }
                if bg != BG && bg != "transparent" {
                    s.push_str(&format!(
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{bg}\"/>
",
                        start as f64 * W,
                        y as f64 * H,
                        (x - start) as f64 * W,
                        H
                    ));
                }
            }
            let mut x = 0;
            while x < cols {
                let c = &buf[(x, y)];
                let key = (color(c.fg, "#dcdce1"), c.modifier.contains(Modifier::BOLD));
                let start = x;
                let mut text = String::new();
                while x < cols {
                    let c = &buf[(x, y)];
                    if (color(c.fg, "#dcdce1"), c.modifier.contains(Modifier::BOLD)) != key {
                        break;
                    }
                    text.push_str(c.symbol());
                    x += 1;
                }
                if text.trim().is_empty() {
                    continue;
                }
                let weight = if key.1 { " font-weight=\"bold\"" } else { "" };
                s.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" textLength=\"{:.1}\" lengthAdjust=\"spacingAndGlyphs\" fill=\"{}\"{weight} xml:space=\"preserve\">{}</text>
",
                    start as f64 * W,
                    y as f64 * H + 13.5,
                    (x - start) as f64 * W,
                    key.0,
                    esc(&text)
                ));
            }
        }
        s.push_str(
            "</g>
</svg>
",
        );
        s
    }

    fn html(buf: &Buffer) -> String {
        let mut s = String::from("<pre style=\"font:14px/1.3 'Cascadia Mono',Consolas,monospace;background:#16161c;color:#dcdce1;padding:12px;margin:0;white-space:pre\">");
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let c = &buf[(x, y)];
                let (mut fg, mut bg) = (color(c.fg, "#dcdce1"), color(c.bg, "transparent"));
                if c.modifier.contains(Modifier::REVERSED) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let weight = if c.modifier.contains(Modifier::BOLD) { "font-weight:bold;" } else { "" };
                let sym = match c.symbol() {
                    "<" => "&lt;",
                    ">" => "&gt;",
                    "&" => "&amp;",
                    s => s,
                };
                s.push_str(&format!("<span style=\"display:inline-block;width:1ch;height:1.3em;vertical-align:top;color:{fg};background:{bg};{weight}\">{sym}</span>"));
            }
            s.push('\n');
        }
        s.push_str("</pre>");
        s
    }

    #[test]
    #[ignore]
    fn snapshots() {
        let Ok(repo) = std::env::var("GMAP_SNAP") else { return };
        let h = crate::history::load(
            std::path::Path::new(&repo),
            &crate::history::LoadOptions { rev: None, cache: true, progress: false, threads: 8 },
        )
        .unwrap();
        let s = crate::analysis::Settings {
            since: None,
            until: None,
            author: None,
            prefix: None,
            bots: false,
            code_only: true,
            half_life: crate::time::YEAR,
            inactive: crate::time::YEAR,
            max_changeset: 30,
        };
        let f = |d| crate::filter::PathFilter::new(d, &[], &h.attributes).unwrap();
        let filters = [f(true), f(false)];
        let mut app = App::new(h, filters, s);
        let (w, hgt) = (std::env::var("GMAP_W").ok().and_then(|v| v.parse().ok()).unwrap_or(160), 46);
        let mut term = Terminal::new(TestBackend::new(w, hgt)).unwrap();
        let mut page = String::from(
            "<!doctype html><meta charset=utf-8><body style=\"background:#0b0b0e;margin:0;padding:16px\">",
        );
        let views: [(&str, Tab, bool); 7] = [
            ("map", Tab::Map, false),
            ("map, one folder down", Tab::Map, true),
            ("hotspots", Tab::Hotspots, false),
            ("coupling", Tab::Coupling, false),
            ("people", Tab::People, false),
            ("activity", Tab::Activity, false),
            ("help", Tab::Map, false),
        ];
        for (name, tab, deeper) in views {
            app.tab = tab;
            app.help = name == "help";
            if deeper {
                app.map_sel = 0;
                if let Some(c) = app.selected_node() {
                    if app.tree.nodes[c].is_dir() {
                        app.map_root = c;
                    }
                }
            }
            term.draw(|f| draw(f, &mut app)).unwrap();
            page.push_str(&format!("<h3 style=\"color:#888;font:14px sans-serif\">{name}</h3>"));
            page.push_str(&html(term.backend().buffer()));
            if let Ok(dir) = std::env::var("GMAP_SVG_DIR") {
                let file = name.replace([',', ' '], "-").replace("--", "-");
                std::fs::write(format!("{dir}/{file}.svg"), svg(term.backend().buffer())).unwrap();
            }
        }
        let out = std::env::var("GMAP_SNAP_OUT").unwrap_or_else(|_| "target/snapshots.html".into());
        std::fs::write(&out, page).unwrap();
    }
}
