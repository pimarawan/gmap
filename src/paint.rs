use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub type Rgb = (u8, u8, u8);

/// Matplotlib's inferno: perceptually uniform, readable in grayscale and by
/// people with the common kinds of color blindness.
const INFERNO: [Rgb; 11] = [
    (0, 0, 4),
    (22, 11, 57),
    (66, 10, 104),
    (106, 23, 110),
    (147, 38, 103),
    (188, 55, 84),
    (221, 81, 58),
    (243, 120, 25),
    (252, 165, 10),
    (246, 215, 70),
    (252, 255, 164),
];

/// Heat in [0, 1] to a color. The darkest end is skipped so cold areas stay
/// visible on a black terminal.
pub fn heat(t: f64) -> Rgb {
    let t = 0.14 + t.clamp(0.0, 1.0) * 0.78;
    let x = t * (INFERNO.len() - 1) as f64;
    let i = (x.floor() as usize).min(INFERNO.len() - 2);
    let f = x - i as f64;
    let (a, b) = (INFERNO[i], INFERNO[i + 1]);
    let mix = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * f).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// GitHub's contribution greens, from no commits to the busiest days.
pub const GREENS: [Rgb; 5] = [(33, 38, 45), (14, 68, 41), (0, 109, 50), (38, 166, 65), (57, 211, 83)];

/// Black or white, whichever reads better on `bg` (WCAG relative luminance).
pub fn ink(bg: Rgb) -> Rgb {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(bg.0) + 0.7152 * lin(bg.1) + 0.0722 * lin(bg.2);
    if l > 0.22 {
        (12, 12, 16)
    } else {
        (238, 238, 238)
    }
}

pub fn lighten(c: Rgb, f: f64) -> Rgb {
    let m = |x: u8| (x as f64 + (255.0 - x as f64) * f).round() as u8;
    (m(c.0), m(c.1), m(c.2))
}

pub fn darken(c: Rgb, f: f64) -> Rgb {
    let m = |x: u8| (x as f64 * (1.0 - f)).round() as u8;
    (m(c.0), m(c.1), m(c.2))
}

const EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A horizontal bar `width` cells wide, filled to `frac`, in eighths of a cell.
pub fn bar(frac: f64, width: usize) -> String {
    let eighths = (frac.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let mut s = String::with_capacity(width * 3);
    for i in 0..width {
        s.push(EIGHTHS[eighths.saturating_sub(i * 8).min(8)]);
    }
    s
}

/// A sparkline scaled to its own maximum. Zero is a space, so quiet stretches look quiet.
pub fn spark(values: &[u32]) -> String {
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f64;
    values.iter().map(|&v| if v == 0 { ' ' } else { SPARKS[((v as f64 / max) * 7.0).round() as usize] }).collect()
}

/// A bar chart `rows` cells tall, one column per value, top row first.
/// Each cell is split in eighths, so 3 rows give 24 levels.
pub fn columns(values: &[u32], rows: usize) -> Vec<String> {
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f64;
    let heights: Vec<usize> = values.iter().map(|&v| ((v as f64 / max) * (rows * 8) as f64).round() as usize).collect();
    (0..rows)
        .rev()
        .map(|r| {
            heights
                .iter()
                .map(|&h| match h.saturating_sub(r * 8).min(8) {
                    0 if r == 0 && h == 0 => ' ',
                    0 => ' ',
                    k => SPARKS[k - 1],
                })
                .collect()
        })
        .collect()
}

/// The color for a truck factor: one person is a red flag, two or three is
/// thin, more is healthy.
pub fn truck_color(n: usize) -> Rgb {
    match n {
        0 | 1 => (235, 100, 100),
        2 | 3 => (255, 170, 60),
        _ => (90, 200, 120),
    }
}

/// A dot sized by `t` in [0, 1], for the weekday-by-hour chart.
pub fn dot(t: f64) -> &'static str {
    match t {
        t if t <= 0.0 => " ",
        t if t < 0.15 => "·",
        t if t < 0.4 => "•",
        t if t < 0.7 => "●",
        _ => "⬤",
    }
}

pub fn width(s: &str) -> usize {
    s.width()
}

/// Cut `s` to at most `w` columns, keeping the end, which is the useful part of a path.
pub fn tail(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('…');
    out.into_iter().rev().collect()
}

/// Cut `s` to at most `w` columns, keeping the start.
pub fn head(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 1;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// Pad or cut to exactly `w` columns.
pub fn fit(s: &str, w: usize) -> String {
    let s = head(s, w);
    let pad = w.saturating_sub(s.width());
    format!("{s}{}", " ".repeat(pad))
}

/// 12,345
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 1.2k, 34k, 5.6M
pub fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1e3),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars() {
        assert_eq!(bar(0.0, 3), "   ");
        assert_eq!(bar(1.0, 3), "███");
        assert_eq!(bar(0.5, 3), "█▌ ");
    }

    #[test]
    fn truncation() {
        assert_eq!(tail("src/core/flags.rs", 10), "…/flags.rs");
        assert_eq!(head("Andrew Gallant", 8), "Andrew …");
        assert_eq!(fit("ab", 4), "ab  ");
        assert!(width(&tail("日本語のファイル.rs", 9)) <= 9);
    }

    #[test]
    fn column_chart() {
        let c = columns(&[0, 4, 8], 2);
        assert_eq!(c, vec!["  █", " ██"]);
    }

    #[test]
    fn numbers() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(short(1234), "1.2k");
        assert_eq!(short(56_000), "56k");
    }

    #[test]
    fn contrast() {
        assert_eq!(ink((0, 0, 0)), (238, 238, 238));
        assert_eq!(ink((252, 255, 164)), (12, 12, 16));
    }
}
