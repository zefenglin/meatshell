//! WindTerm-style syntax highlighting for plain terminal output.
//!
//! WindTerm does not colour output because the remote sent ANSI codes: its
//! Linux TextMate grammar (terminal/schemes/linux/linux.lexer) marks commands,
//! options, punctuation, strings, permissions, numbers, IP / MAC addresses,
//! timestamps and a four-list keyword table, and the active theme maps each
//! scope to a colour. That is why an "ip addr" dump or an MOTD looks colourful
//! there and plain here (#m07146). This module scans a line for the same kinds
//! of token and gives each one an exact RGB colour; vt_color_to_slint passes
//! RGB through unchanged whenever a scheme is active.
//!
//! Only plain runs are touched (default foreground and background, not bold,
//! inverse or underlined): output from ls --color, git, systemctl and friends
//! already carries its own colours, which win.
//!
//! The port follows linux.lexer closely:
//!
//! * a prompt line (the first "$", "#", ">" or "%" followed by a space, or a
//!   powerline prompt glyph) switches to the rules the grammar nests inside
//!   prompt.line: command, option, operator, string, ip, path, number;
//! * any other line runs the top-level rules: keyword, permissions, option,
//!   operator, string, ip, mac, datetime, number. Paths are deliberately not
//!   part of the top-level set, exactly like the grammar.

use super::presentation::text_cell_width;
use crate::terminal::HistSpan;

/// Scope colours taken from WindTerm's dige-black / dige-white themes.
#[derive(Clone, Copy)]
pub(crate) struct SyntaxPalette {
    /// constant.numeric - numbers.
    number: [u8; 3],
    /// markup.heading - IPv4 / IPv6 / MAC addresses and timestamps.
    heading: [u8; 3],
    /// variable.language - --options.
    option: [u8; 3],
    /// entity.name.filename - paths inside a prompt line.
    path: [u8; 3],
    error: [u8; 3],
    success: [u8; 3],
    warn: [u8; 3],
    info: [u8; 3],
    /// URLs (detected by WindTerm itself, not by the grammar).
    link: [u8; 3],
    /// punctuation.definition - operators and brackets.
    punct: [u8; 3],
    /// string.quoted.double - quoted strings.
    string: [u8; 3],
    /// sign.directive.prompt - the prompt sign itself.
    sign: [u8; 3],
    /// support.function - the command after a prompt sign or a pipe.
    command: [u8; 3],
    /// token.debug-token - the file type letter of a permission string.
    debug: [u8; 3],
    /// meta.diff - the "=" of "--option=value".
    diff: [u8; 3],
    /// support.function is bold in the light theme only.
    command_bold: bool,
}

const DARK: SyntaxPalette = SyntaxPalette {
    number: [0xAE, 0x81, 0xFF],
    heading: [0xA6, 0xE2, 0x2E],
    option: [0xFD, 0x97, 0x1F],
    path: [0xE6, 0xDB, 0x74],
    error: [0xF4, 0x47, 0x47],
    success: [0x32, 0xCD, 0x32],
    warn: [0xCD, 0x97, 0x31],
    info: [0x67, 0x96, 0xE6],
    link: [0x66, 0xD9, 0xEF],
    punct: [0xA6, 0xE2, 0x2E],
    string: [0xE6, 0xDB, 0x74],
    sign: [0xF9, 0x26, 0x72],
    command: [0x66, 0xD9, 0xEF],
    debug: [0xB2, 0x67, 0xE6],
    diff: [0x75, 0x71, 0x5E],
    command_bold: false,
};

const LIGHT: SyntaxPalette = SyntaxPalette {
    number: [0xDD, 0xA0, 0xDD],
    heading: [0x20, 0xB2, 0xAA],
    option: [0xFD, 0x97, 0x1F],
    path: [0xFF, 0x8C, 0x00],
    error: [0xF4, 0x47, 0x47],
    success: [0x32, 0xCD, 0x32],
    warn: [0xCD, 0x97, 0x31],
    info: [0x67, 0x96, 0xE6],
    link: [0x1E, 0x90, 0xFF],
    punct: [0x3C, 0xB3, 0x71],
    string: [0x44, 0x8C, 0x27],
    sign: [0x4B, 0x69, 0xC6],
    command: [0x1E, 0x90, 0xFF],
    debug: [0xB2, 0x67, 0xE6],
    diff: [0x75, 0x71, 0x5E],
    command_bold: true,
};

pub(crate) fn palette(is_dark: bool) -> &'static SyntaxPalette {
    if is_dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// One highlighted slice of a line, in byte offsets.
#[derive(Clone, Copy)]
struct Mark {
    start: usize,
    end: usize,
    colour: [u8; 3],
    underline: bool,
    bold: bool,
}

fn mark(start: usize, end: usize, colour: [u8; 3], underline: bool, bold: bool) -> Mark {
    Mark {
        start,
        end,
        colour,
        underline,
        bold,
    }
}

/// Colour every recognised token of every plain run. Runs that already carry
/// a style are returned untouched.
pub(crate) fn highlight_syntax(runs: Vec<HistSpan>, palette: &SyntaxPalette) -> Vec<HistSpan> {
    if runs.is_empty() {
        return runs;
    }

    // Prompt rules need the characters before a run (the prompt sign and the
    // command usually arrive as separate ANSI runs), so the line is assembled
    // once and every mark is mapped back onto the run it falls into.
    let mut line = String::with_capacity(runs.iter().map(|r| r.text.len()).sum());
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(runs.len());
    let mut contiguous = true;
    let mut expected = runs[0].col;
    for run in &runs {
        if run.col != expected {
            contiguous = false;
        }
        expected = run.col + text_cell_width(&run.text);
        spans.push((line.len(), line.len() + run.text.len()));
        line.push_str(&run.text);
    }

    let mut out = Vec::with_capacity(runs.len() + 4);
    if !contiguous {
        // A gap (wide cells, a skipped column) means the pieces no longer
        // describe one line: highlight each run on its own, as before.
        for (idx, run) in runs.iter().enumerate() {
            let (rs, re) = spans[idx];
            let slice = &line[rs..re];
            let marks = scan_output(slice, palette);
            paint_run(run, slice, 0, slice.len(), &marks, &mut out);
        }
        return out;
    }

    let prompt = prompt_span(&line);
    let marks = match &prompt {
        Some(prompt) => scan_prompt(&line, prompt, palette),
        None => scan_output(&line, palette),
    };
    for (idx, run) in runs.iter().enumerate() {
        let (rs, re) = spans[idx];
        paint_run(run, &line, rs, re, &marks, &mut out);
    }
    out
}

/// Clip the line-wide marks that fall inside `rs..re` onto one run.
fn paint_run(
    run: &HistSpan,
    line: &str,
    rs: usize,
    re: usize,
    marks: &[Mark],
    out: &mut Vec<HistSpan>,
) {
    if !is_plain(run) || rs == re {
        out.push(run.clone());
        return;
    }
    let first = marks.partition_point(|m| m.end <= rs);
    let mut cur = first;
    let mut cursor = rs;
    let mut col = run.col;
    let mut painted = false;
    while cur < marks.len() && marks[cur].start < re {
        let m = marks[cur];
        let s = m.start.max(rs);
        let e = m.end.min(re);
        if s > cursor {
            push_plain(run, &line[cursor..s], col, out);
            col += text_cell_width(&line[cursor..s]);
        }
        if e > s {
            push_styled(run, &line[s..e], col, m, out);
            col += text_cell_width(&line[s..e]);
            painted = true;
        }
        cursor = e;
        if m.end <= re {
            cur += 1;
        } else {
            break;
        }
    }
    if !painted && cursor == rs {
        out.push(run.clone());
        return;
    }
    if cursor < re {
        push_plain(run, &line[cursor..re], col, out);
    }
}

fn push_plain(run: &HistSpan, text: &str, col: i32, out: &mut Vec<HistSpan>) {
    let mut piece = run.clone();
    piece.text = text.to_string();
    piece.col = col;
    piece.cells = text_cell_width(text).max(1);
    out.push(piece);
}

fn push_styled(run: &HistSpan, text: &str, col: i32, m: Mark, out: &mut Vec<HistSpan>) {
    let mut piece = run.clone();
    piece.text = text.to_string();
    piece.col = col;
    piece.cells = text_cell_width(text).max(1);
    piece.fg = vt100::Color::Rgb(m.colour[0], m.colour[1], m.colour[2]);
    piece.underline = m.underline;
    piece.bold = m.bold;
    out.push(piece);
}

fn is_plain(run: &HistSpan) -> bool {
    matches!(run.fg, vt100::Color::Default)
        && matches!(run.bg, vt100::Color::Default)
        && !run.bold
        && !run.inverse
        && !run.underline
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn char_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

/// Where a prompt sign sits inside the line.
struct Prompt {
    sign_start: usize,
    sign_end: usize,
    body_start: usize,
}

/// The lexer's prompt.line pattern: the first "$", "#", ">" or "%" (not a
/// percentage) followed by a space, or a powerline prompt glyph, on a line
/// that does not start like diff output.
fn prompt_span(line: &str) -> Option<Prompt> {
    let b = line.as_bytes();
    if b.is_empty() || b[0] == b'-' || b[0] == b'+' {
        return None;
    }
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        let glyph = c == 0xEE && b.get(i + 1) == Some(&0x82) && b.get(i + 2) == Some(&0xB0);
        let sign = matches!(c, b'$' | b'#' | b'>' | b'%') && b.get(i + 1) == Some(&b' ');
        if glyph {
            let end = i + 3;
            if b.get(end) == Some(&b' ') {
                return Some(Prompt {
                    sign_start: i,
                    sign_end: end,
                    body_start: end,
                });
            }
        } else if sign {
            if c == b'%' && percent_sign(&b[..i]) {
                i += 1;
                continue;
            }
            // "<UP,LOWER_UP>" is a flag list, not a prompt: the ">" closes a
            // bracketed word that has no space inside it.
            if c == b'>' && angle_wrapped(b, i) {
                i += 1;
                continue;
            }
            return Some(Prompt {
                sign_start: i,
                sign_end: i + 1,
                body_start: i + 1,
            });
        }
        i += 1;
    }
    None
}

/// True when byte `i` is a ">" that closes an angle-bracket run such as
/// "<BROADCAST,MULTICAST,UP,LOWER_UP>".
fn angle_wrapped(b: &[u8], i: usize) -> bool {
    let mut k = i;
    while k > 0 {
        k -= 1;
        match b[k] {
            b' ' | b'\t' => return false,
            b'<' => return true,
            _ => {}
        }
    }
    false
}

/// "%" right after a number is a percentage, not a prompt.
fn percent_sign(before: &[u8]) -> bool {
    let mut k = before.len();
    let mut digits = 0usize;
    while k > 0 && digits < 3 && before[k - 1].is_ascii_digit() {
        k -= 1;
        digits += 1;
    }
    digits > 0 && k > 0 && (before[k - 1] == b' ' || before[k - 1] == b'\t')
}

fn command_trigger(b: &[u8], i: usize, body_start: usize) -> bool {
    if i == body_start {
        return true;
    }
    if i == 0 {
        return false;
    }
    matches!(b[i - 1], b'|' | b'$' | b'#' | b'>')
        || (i >= 3 && b[i - 3] == 0xEE && b[i - 2] == 0x82 && b[i - 1] == 0xB0)
}

/// support.function: the command word after the prompt sign or a pipe.
fn match_command(b: &[u8], i: usize, body_start: usize) -> Option<(usize, usize)> {
    if !command_trigger(b, i, body_start) {
        return None;
    }
    let mut j = i;
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    if j >= b.len() {
        return None;
    }
    let mut k = j;
    let mut digits = 0usize;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
        digits += 1;
    }
    if digits > 0 && k < b.len() && b[k] == b'.' {
        return None;
    }
    let start = j;
    while j < b.len() && (is_word(b[j]) || b[j] == b'.' || b[j] == b'-') {
        j += 1;
    }
    if j == start {
        return None;
    }
    if j < b.len() && b[j] != b' ' && b[j] != b'\t' {
        return None;
    }
    Some((start, j))
}

fn scan_prompt(text: &str, prompt: &Prompt, p: &SyntaxPalette) -> Vec<Mark> {
    let b = text.as_bytes();
    let mut marks: Vec<Mark> = Vec::with_capacity(6);
    marks.push(mark(
        prompt.sign_start,
        prompt.sign_end,
        p.sign,
        false,
        false,
    ));
    let mut i = prompt.body_start;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            i += char_len(c);
            continue;
        }
        if let Some((start, end)) = match_command(b, i, prompt.body_start) {
            marks.push(mark(start, end, p.command, false, p.command_bold));
            i = end;
            continue;
        }
        let boundary = i == 0 || !is_word(b[i - 1]);
        if boundary {
            if let Some((end, pieces)) = match_option(b, i, p) {
                marks.extend(pieces);
                i = end;
                continue;
            }
        }
        if let Some(m) = match_operator(b, i, p) {
            marks.push(m);
            i += 1;
            continue;
        }
        if let Some(end) = match_string(b, i) {
            marks.push(mark(i, end, p.string, false, false));
            i = end;
            continue;
        }
        if boundary {
            if let Some(end) = match_ipv4(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_ipv6(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
        }
        if let Some(end) = match_path(b, i) {
            marks.push(mark(i, end, p.path, false, false));
            i = end;
            continue;
        }
        if boundary {
            if let Some(end) = match_number(b, i) {
                marks.push(mark(i, end, p.number, false, false));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    marks
}

fn scan_output(text: &str, p: &SyntaxPalette) -> Vec<Mark> {
    let b = text.as_bytes();
    let mut marks: Vec<Mark> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c >= 0x80 {
            i += char_len(c);
            continue;
        }
        let boundary = i == 0 || !is_word(b[i - 1]);
        if boundary {
            if let Some(end) = match_url(b, i) {
                marks.push(mark(i, end, p.link, true, false));
                i = end;
                continue;
            }
            if let Some((end, colour)) = match_keyword(b, i, p) {
                marks.push(mark(i, end, colour, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_permissions(b, i) {
                push_permissions(b, i, end, p, &mut marks);
                i = end;
                continue;
            }
            if let Some((end, pieces)) = match_option(b, i, p) {
                marks.extend(pieces);
                i = end;
                continue;
            }
        }
        if let Some(m) = match_operator(b, i, p) {
            marks.push(m);
            i += 1;
            continue;
        }
        if let Some(end) = match_string(b, i) {
            marks.push(mark(i, end, p.string, false, false));
            i = end;
            continue;
        }
        if boundary {
            if let Some(end) = match_ipv4(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_ipv6(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_mac(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_datetime(b, i) {
                marks.push(mark(i, end, p.heading, false, false));
                i = end;
                continue;
            }
            if let Some(end) = match_number(b, i) {
                marks.push(mark(i, end, p.number, false, false));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    marks
}

// --- Token matchers --------------------------------------------------------

fn match_url(b: &[u8], i: usize) -> Option<usize> {
    const SCHEMES: [&[u8]; 4] = [b"https://", b"http://", b"ftp://", b"www."];
    let scheme = SCHEMES.iter().find(|s| b[i..].starts_with(s))?;
    let mut j = i + scheme.len();
    while j < b.len() {
        let c = b[j];
        if c <= b' ' || matches!(c, b'"' | 0x27 | b'<' | b'>' | 0x60 | b'|') {
            break;
        }
        j += 1;
    }
    // Trailing sentence punctuation is not part of the link.
    while j > i + 1 && matches!(b[j - 1], b'.' | b',' | b';' | b':' | b')' | b']' | b'}') {
        j -= 1;
    }
    (j > i + scheme.len()).then_some(j)
}

fn word_end(b: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < b.len() && is_word(b[j]) {
        j += 1;
    }
    j
}

/// Case-insensitive whole-word match; returns the end offset.
fn ci_word(b: &[u8], i: usize, word: &[u8]) -> Option<usize> {
    let end = i + word.len();
    if end > b.len() || !b[i..end].eq_ignore_ascii_case(word) {
        return None;
    }
    if end < b.len() && is_word(b[end]) {
        return None;
    }
    Some(end)
}

/// The one optional word the keyword patterns allow after them.
fn extra_word(b: &[u8], end: usize) -> usize {
    if end + 1 < b.len() && b[end] == b' ' && is_word(b[end + 1]) {
        word_end(b, end + 1)
    } else {
        end
    }
}

const AUX: [&[u8]; 7] = [b"do", b"does", b"can", b"will", b"could", b"should", b"would"];

const CONTRACTIONS: [(&[u8], &[u8]); 7] = [
    (b"don", b"'t"),
    (b"doesn", b"'t"),
    (b"can", b"'t"),
    (b"won", b"'t"),
    (b"couldn", b"'t"),
    (b"shouldn", b"'t"),
    (b"wouldn", b"'t"),
];

fn match_keyword(b: &[u8], i: usize, p: &SyntaxPalette) -> Option<(usize, [u8; 3])> {
    if !b[i].is_ascii_alphabetic() {
        return None;
    }
    // "<aux> not [word]"
    for aux in AUX {
        if let Some(after) = ci_word(b, i, aux) {
            if after < b.len() && b[after] == b' ' {
                if let Some(not_end) = ci_word(b, after + 1, b"not") {
                    return Some((extra_word(b, not_end), p.error));
                }
            }
        }
    }
    // "don't [be] [word]"
    for (stem, tail) in CONTRACTIONS {
        if let Some(after) = ci_word(b, i, stem) {
            let tail_end = after + tail.len();
            if tail_end <= b.len() && b[after..tail_end].eq_ignore_ascii_case(tail) {
                let mut end = tail_end;
                if end < b.len() && b[end] == b' ' {
                    if let Some(be_end) = ci_word(b, end + 1, b"be") {
                        end = extra_word(b, be_end);
                    } else {
                        end = extra_word(b, end);
                    }
                }
                return Some((end, p.error));
            }
        }
    }
    let end = word_end(b, i);
    let word = std::str::from_utf8(&b[i..end]).ok()?.to_ascii_lowercase();
    let colour = if is_in(&word, ERROR) {
        p.error
    } else if is_in(&word, SUCCESS) {
        p.success
    } else if is_in(&word, WARN) {
        p.warn
    } else if is_in(&word, INFO) {
        p.info
    } else {
        return None;
    };
    let extended = (colour == p.error && matches!(word.as_str(), "cannot" | "no" | "not"))
        || (colour == p.success && word == "can");
    let span_end = if extended { extra_word(b, end) } else { end };
    Some((span_end, colour))
}

/// "[bcdCdDlMnpPs?-]([rw-]{2}[xsStT-]){3}[.+ ]" starting on a word boundary.
fn match_permissions(b: &[u8], i: usize) -> Option<usize> {
    if i > 0 && !(b[i - 1] == b' ' || b[i - 1] == b'\t') {
        return None;
    }
    if !matches!(
        b[i],
        b'b' | b'c' | b'C' | b'd' | b'D' | b'l' | b'M' | b'n' | b'p' | b'P' | b's' | b'?' | b'-'
    ) {
        return None;
    }
    let mut j = i + 1;
    for _ in 0..3 {
        if j + 3 > b.len() {
            return None;
        }
        if !matches!(b[j], b'r' | b'w' | b'-') {
            return None;
        }
        if !matches!(b[j + 1], b'r' | b'w' | b'-') {
            return None;
        }
        if !matches!(b[j + 2], b'x' | b's' | b'S' | b't' | b'T' | b'-') {
            return None;
        }
        j += 3;
    }
    if j < b.len() && (b[j] == b'.' || b[j] == b'+') {
        j += 1;
    }
    if j < b.len() && b[j] == b' ' {
        Some(j)
    } else {
        None
    }
}

fn push_permissions(b: &[u8], start: usize, end: usize, p: &SyntaxPalette, marks: &mut Vec<Mark>) {
    let mut i = start;
    while i < end {
        let colour = match b[i] {
            b'b' | b'c' | b'd' | b'l' | b'p' => Some(p.debug),
            b'r' => Some(p.info),
            b'w' => Some(p.warn),
            b'x' | b's' | b'S' | b't' | b'T' => Some(p.error),
            b'-' => Some(p.punct),
            _ => None,
        };
        if let Some(colour) = colour {
            if let Some(last) = marks.last_mut() {
                if last.end == i && last.colour == colour && !last.underline && !last.bold {
                    last.end = i + 1;
                    i += 1;
                    continue;
                }
            }
            marks.push(mark(i, i + 1, colour, false, false));
        }
        i += 1;
    }
}

fn match_option(b: &[u8], i: usize, p: &SyntaxPalette) -> Option<(usize, Vec<Mark>)> {
    if b[i] != b'-' {
        return None;
    }
    let allowed_left = i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'[' | b'|');
    if !allowed_left {
        return None;
    }
    let mut j = i + 1;
    if j < b.len() && b[j] == b'-' {
        j += 1;
    }
    if j >= b.len() || !is_word(b[j]) {
        return None;
    }
    while j < b.len() && (is_word(b[j]) || b[j] == b'-') {
        j += 1;
    }
    let mut eq = None;
    let mut value_end = j;
    if j + 1 < b.len() && b[j] == b'=' && is_word(b[j + 1]) {
        eq = Some(j);
        let mut k = j + 1;
        while k < b.len() && is_word(b[k]) {
            k += 1;
        }
        value_end = k;
    }
    if value_end < b.len() && is_word(b[value_end]) {
        return None;
    }
    let mut pieces = Vec::with_capacity(3);
    match eq {
        Some(e) => {
            pieces.push(mark(i, e, p.option, false, false));
            pieces.push(mark(e, e + 1, p.diff, false, false));
            pieces.push(mark(e + 1, value_end, p.info, false, false));
        }
        None => pieces.push(mark(i, value_end, p.option, false, false)),
    }
    Some((value_end, pieces))
}

fn match_operator(b: &[u8], i: usize, p: &SyntaxPalette) -> Option<Mark> {
    let c = b[i];
    let is_operator = matches!(
        c,
        b'=' | b':' | b';' | b'|' | b'?' | b'*' | b'$' | b'<' | b'>' | b'&' | b'+' | b'-'
    ) || matches!(c, b'(' | b')' | b'[' | b']' | b'{' | b'}');
    if is_operator {
        Some(mark(i, i + 1, p.punct, false, false))
    } else {
        None
    }
}

fn match_string(b: &[u8], i: usize) -> Option<usize> {
    let quote = b[i];
    if quote != 0x27 && quote != b'"' {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && b[j] != quote {
        j += 1;
    }
    if j < b.len() {
        Some(j + 1)
    } else {
        Some(b.len())
    }
}

/// Characters the grammar lets a path contain besides "/".
fn is_path_char(c: u8) -> bool {
    (0x21..=0x2E).contains(&c) || c >= 0x30
}

/// The prompt-only path rule: a run of path characters that contains a slash.
fn match_path(b: &[u8], i: usize) -> Option<usize> {
    if b[i] != b'/' && !is_path_char(b[i]) {
        return None;
    }
    let mut j = i;
    let mut slash = false;
    while j < b.len() && (is_path_char(b[j]) || b[j] == b'/') {
        if b[j] == b'/' {
            slash = true;
        }
        j += 1;
    }
    if !slash || j == i {
        return None;
    }
    Some(j)
}

fn match_ipv4(b: &[u8], i: usize) -> Option<usize> {
    if !b[i].is_ascii_digit() {
        return None;
    }
    let mut j = i;
    for part in 0..4 {
        let start = j;
        let mut value = 0u32;
        while j < b.len() && b[j].is_ascii_digit() && j - start < 3 {
            value = value * 10 + u32::from(b[j] - b'0');
            j += 1;
        }
        if j == start || value > 255 {
            return None;
        }
        if part < 3 {
            if j >= b.len() || b[j] != b'.' {
                return None;
            }
            j += 1;
        }
    }
    if j < b.len() && (is_word(b[j]) || b[j] == b'.') {
        return None;
    }
    Some(j)
}

/// Hex groups separated by colons. Short groups are only accepted when the
/// address uses "::" or a full-width group, so clock times stay plain.
fn match_ipv6(b: &[u8], i: usize) -> Option<usize> {
    // The grammar wraps the address in \b, so it can neither start nor end on
    // a colon: "::1" stays plain while "fe80::1" is highlighted.
    if !b[i].is_ascii_hexdigit() {
        return None;
    }
    let mut j = i;
    let mut groups = 0usize;
    let mut group_len = 0usize;
    let mut longest = 0usize;
    let mut colons = 0usize;
    let mut compressed = false;
    while j < b.len() && (b[j].is_ascii_hexdigit() || b[j] == b':') {
        if b[j] == b':' {
            colons += 1;
            if group_len > 0 {
                groups += 1;
                longest = longest.max(group_len);
                group_len = 0;
            } else if colons >= 2 {
                compressed = true;
            }
        } else {
            group_len += 1;
            if group_len > 4 {
                return None;
            }
        }
        j += 1;
    }
    if group_len > 0 {
        groups += 1;
        longest = longest.max(group_len);
    }
    if groups == 0 || colons < 2 {
        return None;
    }
    if !(compressed || longest == 4 || groups >= 6) {
        return None;
    }
    if j < b.len() && (is_word(b[j]) || b[j] == b'.' || b[j] == b':') {
        return None;
    }
    if j > i && b[j - 1] == b':' {
        return None;
    }
    Some(j)
}

fn match_mac(b: &[u8], i: usize) -> Option<usize> {
    if i + 17 > b.len() {
        return None;
    }
    let sep = b[i + 2];
    if sep != b':' && sep != b'-' {
        return None;
    }
    for k in 0..6 {
        let at = i + k * 3;
        if !b[at].is_ascii_hexdigit() || !b[at + 1].is_ascii_hexdigit() {
            return None;
        }
        if k < 5 && b[at + 2] != sep {
            return None;
        }
    }
    let end = i + 17;
    if end < b.len() && (is_word(b[end]) || b[end] == sep || b[end] == b'.') {
        return None;
    }
    Some(end)
}

fn match_datetime(b: &[u8], i: usize) -> Option<usize> {
    if b[i].is_ascii_digit() {
        let run = |mut j: usize, max: usize| -> usize {
            let start = j;
            while j < b.len() && b[j].is_ascii_digit() && j - start < max {
                j += 1;
            }
            j
        };
        // yyyy-mm-dd
        let j = run(i, 4);
        if j - i == 4 && j < b.len() && (b[j] == b'-' || b[j] == b'/') {
            let k = run(j + 1, 2);
            if k > j + 1 && k < b.len() && (b[k] == b'-' || b[k] == b'/') {
                let m = run(k + 1, 2);
                if m > k + 1 {
                    return Some(extend_time(b, m));
                }
            }
        }
        // d/m/yy or d-m-yyyy
        let k = run(i, 2);
        if k > i && k < b.len() && (b[k] == b'-' || b[k] == b'/') {
            let m = run(k + 1, 2);
            if m > k + 1 && m < b.len() && (b[m] == b'-' || b[m] == b'/') {
                let n = run(m + 1, 4);
                if n > m + 1 {
                    return Some(extend_time(b, n));
                }
            }
        }
        // hh:mm(:ss)
        let k = run(i, 2);
        if k > i && k + 2 < b.len() && b[k] == b':' {
            let m = run(k + 1, 2);
            if m - (k + 1) == 2 {
                let mut end = m;
                if end + 2 < b.len() && b[end] == b':' {
                    let s = run(end + 1, 2);
                    if s - (end + 1) == 2 {
                        end = s;
                    }
                }
                if end < b.len() && b[end].is_ascii_digit() {
                    return None;
                }
                return Some(end);
            }
        }
        return None;
    }
    if !b[i].is_ascii_alphabetic() {
        return None;
    }
    // Month and weekday names are part of the grammar's datetime rule.
    const NAMES: [&[u8]; 26] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Sept", b"Oct",
        b"Nov", b"Dec", b"Mon", b"Tue", b"Tues", b"Wed", b"Thu", b"Thur", b"Fri", b"Sat", b"Sun",
        b"am", b"pm", b"GMT", b"UTC",
    ];
    let mut best: Option<usize> = None;
    for name in NAMES {
        if b.len() - i >= name.len() && b[i..i + name.len()].eq_ignore_ascii_case(name) {
            let end = i + name.len();
            if end < b.len() && b[end].is_ascii_alphabetic() {
                continue;
            }
            best = Some(best.map_or(end, |x: usize| x.max(end)));
        }
    }
    best
}

fn extend_time(b: &[u8], from: usize) -> usize {
    if from + 3 <= b.len() && (b[from] == b' ' || b[from] == b'T') && b[from + 1].is_ascii_digit() {
        let mut j = from + 1;
        let mut colons = 0;
        while j < b.len() && (b[j].is_ascii_digit() || b[j] == b':') {
            if b[j] == b':' {
                colons += 1;
            }
            j += 1;
        }
        if colons >= 1 {
            return j;
        }
    }
    from
}

fn match_number(b: &[u8], i: usize) -> Option<usize> {
    if !b[i].is_ascii_digit() {
        return None;
    }
    // The grammar first looks for hex: after whitespace, 0x.. or four / eight
    // hex digits on their own.
    if i == 0 || b[i - 1].is_ascii_whitespace() {
        if b[i] == b'0' && i + 2 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > i + 2 && (j == b.len() || !is_word(b[j])) {
                return Some(j);
            }
        }
        let mut run = 0usize;
        while run < 8 && i + run < b.len() && b[i + run].is_ascii_hexdigit() {
            run += 1;
        }
        if run == 8 && (i + 8 == b.len() || !is_word(b[i + 8])) {
            return Some(i + 8);
        }
        if run >= 4 && (i + 4 == b.len() || !is_word(b[i + 4])) {
            return Some(i + 4);
        }
    }
    if i >= 2 && b[i - 1] == b'-' && b[i - 2] != b' ' && b[i - 2] != b'\t' {
        return None;
    }
    if i > 0 && (b[i - 1] == b'.' || b[i - 1].is_ascii_digit()) {
        return None;
    }
    let mut j = i;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
        j += 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
    }
    if j + 1 < b.len() && (b[j] == b'e' || b[j] == b'E') && (b[j + 1] == b'+' || b[j + 1] == b'-')
    {
        let mut k = j + 2;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k > j + 2 {
            j = k;
        }
    }
    if j < b.len() && b[j] == b'%' {
        j += 1;
    }
    if j < b.len() && (is_word(b[j]) || matches!(b[j], b'.' | b':' | b'-' | b'/')) {
        return None;
    }
    Some(j)
}

fn is_in(word: &str, list: &[&str]) -> bool {
    list.binary_search(&word).is_ok()
}

// Sorted for binary_search.
const ERROR: &[&str] = &[
    "bad", "cannot", "denied", "deprecated", "disabled", "error", "errors", "fail", "failed",
    "failure", "false", "important", "incorrect", "invalid", "no", "none", "not", "refused",
    "unknown", "unsupported", "warn", "warning", "wrong",
];
const SUCCESS: &[&str] = &[
    "can", "correct", "correctly", "known", "ok", "pass", "passed", "success", "successful",
    "successfully", "supported", "true", "valid", "yes",
];
const WARN: &[&str] = &[
    "closed", "debug", "disconnected", "exited", "skipped", "stopped", "sudo", "terminated",
];
const INFO: &[&str] = &[
    "access", "any", "authentication", "connection", "disconnection", "info", "login",
    "operation", "password", "permission",
];

#[cfg(test)]
mod tests {
    use super::*;
    use vt100::Color;

    fn run(text: &str) -> HistSpan {
        HistSpan {
            text: text.to_string(),
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            inverse: false,
            underline: false,
            col: 0,
            cells: text_cell_width(text),
        }
    }

    fn coloured(text: &str) -> Vec<(String, Color)> {
        let out = highlight_syntax(vec![run(text)], palette(true));
        out.iter().map(|s| (s.text.clone(), s.fg)).collect()
    }

    fn find(text: &str, token: &str) -> Option<Color> {
        coloured(text)
            .into_iter()
            .find(|(t, _)| t == token)
            .map(|(_, c)| c)
    }

    fn rgb(r: u8, g: u8, b: u8) -> Option<Color> {
        Some(Color::Rgb(r, g, b))
    }

    #[test]
    fn colours_addresses_numbers_and_paths() {
        let line = "inet 192.168.1.10/24 brd 192.168.1.255 scope global eth0";
        assert_eq!(find(line, "192.168.1.10"), rgb(0xA6, 0xE2, 0x2E));
        assert_eq!(find(line, "/24"), None);
        assert_eq!(find(line, "24"), rgb(0xAE, 0x81, 0xFF));
        let mac = "link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff";
        assert_eq!(find(mac, "52:54:00:12:34:56"), rgb(0xA6, 0xE2, 0x2E));
        // Paths are only highlighted inside prompt lines, like the grammar.
        assert_eq!(
            find("/etc/ssh/sshd_config", "/etc/ssh/sshd_config"),
            Some(Color::Default)
        );
        assert_eq!(find("uptime 12345 seconds", "12345"), rgb(0xAE, 0x81, 0xFF));
        // Version strings stay plain: colouring "5", "15" and "0" separately
        // would shred "Linux 5.15.0-91-generic".
        assert_eq!(find("Linux 5.15.0-91-generic", "5"), None);
    }

    #[test]
    fn clock_times_are_not_addresses() {
        let out = coloured("started at 10:30:45 today");
        assert_eq!(
            find("started at 10:30:45 today", "10:30:45"),
            rgb(0xA6, 0xE2, 0x2E)
        );
        assert!(out.iter().any(|(t, _)| t.trim_end() == "started at"));
        assert_eq!(find("2001:db8::1", "2001:db8::1"), rgb(0xA6, 0xE2, 0x2E));
    }

    #[test]
    fn keywords_use_their_own_colours() {
        assert_eq!(
            find("Connection refused", "Connection"),
            rgb(0x67, 0x96, 0xE6)
        );
        assert_eq!(find("Connection refused", "refused"), rgb(0xF4, 0x47, 0x47));
        assert_eq!(find("login success", "success"), rgb(0x32, 0xCD, 0x32));
        assert_eq!(find("connection closed", "closed"), rgb(0xCD, 0x97, 0x31));
        assert_eq!(find("connecting", "connecting"), Some(Color::Default));
        assert_eq!(find("no such file", "no such"), rgb(0xF4, 0x47, 0x47));
        assert_eq!(find("do not panic", "do not panic"), rgb(0xF4, 0x47, 0x47));
        assert_eq!(find("don't worry", "don't worry"), rgb(0xF4, 0x47, 0x47));
    }

    #[test]
    fn options_urls_and_coloured_output() {
        assert_eq!(
            find("apt list --upgradable", "--upgradable"),
            rgb(0xFD, 0x97, 0x1F)
        );
        assert_eq!(find("ls -la /tmp", "-la"), rgb(0xFD, 0x97, 0x1F));
        assert_eq!(find("ls -la /tmp", "/tmp"), None);
        assert_eq!(
            find(
                "see https://docs.ubuntu.com for details",
                "https://docs.ubuntu.com"
            ),
            rgb(0x66, 0xD9, 0xEF)
        );
        let mut painted = run("error: 123");
        painted.fg = Color::Idx(1);
        let out = highlight_syntax(vec![painted], palette(true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "error: 123");
    }

    #[test]
    fn prompt_lines_colour_the_sign_and_the_command() {
        let line = "root@host:~# ip add";
        assert_eq!(find(line, "#"), rgb(0xF9, 0x26, 0x72));
        assert_eq!(find(line, "ip"), rgb(0x66, 0xD9, 0xEF));
        assert_eq!(find(line, "add"), None);
        assert_eq!(
            find("root@host:~# cat /etc/hosts", "/etc/hosts"),
            rgb(0xE6, 0xDB, 0x74)
        );
        assert_eq!(
            find("root@host:~# ls -la", "-la"),
            rgb(0xFD, 0x97, 0x1F)
        );
    }

    #[test]
    fn permissions_strings_and_operators() {
        let out = coloured("-rwxr-xr-x 1 root root");
        assert_eq!(out[0].0, "-");
        assert_eq!(out[0].1, Color::Rgb(0xA6, 0xE2, 0x2E));
        assert_eq!(out[1].0, "r");
        assert_eq!(out[1].1, Color::Rgb(0x67, 0x96, 0xE6));
        assert_eq!(out[2].0, "w");
        assert_eq!(out[2].1, Color::Rgb(0xCD, 0x97, 0x31));
        assert_eq!(out[3].0, "x");
        assert_eq!(out[3].1, Color::Rgb(0xF4, 0x47, 0x47));
        assert_eq!(
            find("echo \"hello world\"", "\"hello world\""),
            rgb(0xE6, 0xDB, 0x74)
        );
        assert_eq!(find("key=value", "="), rgb(0xA6, 0xE2, 0x2E));
    }

    #[test]
    fn flag_lists_are_not_prompts() {
        let line = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000";
        assert_eq!(find(line, "UNKNOWN"), rgb(0xF4, 0x47, 0x47));
        assert_eq!(find(line, "65536"), rgb(0xAE, 0x81, 0xFF));
        let command = coloured(line)
            .into_iter()
            .any(|(_, c)| c == Color::Rgb(0x66, 0xD9, 0xEF));
        assert!(!command, "mtu must not be treated as a prompt command");
        // Real prompts still work.
        assert_eq!(
            find("root@host:~# ip add", "ip"),
            rgb(0x66, 0xD9, 0xEF)
        );
    }

    #[test]
    fn bare_double_colon_is_not_an_address() {
        // The grammar wraps addresses in \b, so "::1" cannot match the IP rule;
        // its colons fall through to the operator rule like any other colon.
        let spans = coloured("inet6 ::1/128 scope host");
        assert!(spans.iter().all(|(t, _)| !t.contains("::1")));
        assert_eq!(
            find("inet6 fe80::4b1:2fff:fe5b:3dcb/64 scope link", "fe80::4b1:2fff:fe5b:3dcb"),
            rgb(0xA6, 0xE2, 0x2E)
        );
    }
}


