//! Multi-line column editing for the docked command editor.
//!
//! The editor's `TextInput` reports its caret as a byte offset into the whole
//! text (`cursor-position-byte-offset`). Everything here takes that single
//! value as the source of truth: the caret byte decides the active line and
//! character column, the anchor line is the fixed end of the column block, and
//! each edit is replayed on every line of the block at the same column.
//!
//! Kept free of UI types so the rules stay unit-testable.

/// Alt+Shift+ArrowDown — start or extend the column block downwards.
pub const OP_SEL_DOWN: i32 = 0;
/// Alt+Shift+ArrowUp — start or extend the column block upwards.
pub const OP_SEL_UP: i32 = 1;
/// ArrowDown while the block is active — move the caret end down.
pub const OP_MOVE_DOWN: i32 = 2;
/// ArrowUp while the block is active — move the caret end up.
pub const OP_MOVE_UP: i32 = 3;
/// ArrowLeft while the block is active — move the insertion column left.
pub const OP_MOVE_LEFT: i32 = 4;
/// ArrowRight while the block is active — move the insertion column right.
pub const OP_MOVE_RIGHT: i32 = 5;
/// A printable character was typed: insert it on every line of the block.
pub const OP_INSERT: i32 = 6;
/// Backspace: delete before the column on every line; at column 0, join lines.
pub const OP_BACKSPACE: i32 = 7;
/// Delete: delete at the column on every line; at EOL, join with the next line.
pub const OP_DELETE: i32 = 8;
/// Esc: leave column mode (the text is untouched).
pub const OP_CANCEL: i32 = 9;
/// Shift+Enter: insert a line break at the caret (or on every line of the
/// block, which is what column editing needs to stay aligned).
pub const OP_NEWLINE: i32 = 10;

/// Resolve (line, column) to a caret byte offset: used by the mouse to place
/// the caret on a click and to drag a rectangle. The text is not touched.
pub const OP_PLACE: i32 = 11;

/// The inverse of `OP_PLACE`: report the caret's own (line, column) instead of
/// moving it. A Shift+Alt click starts its rectangle at the caret, so the mouse
/// needs to know where that is before it takes the caret over.
pub const OP_LOCATE: i32 = 12;

/// Column mode is off; the caret/anchor pair is carried through unchanged.
pub const NO_ANCHOR: i32 = -1;

/// Outcome of one editor operation, ready to be written straight back into the
/// `TextInput` (text + caret offset) and into the column-mode state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    pub text: String,
    /// Caret as a byte offset into `text` (same domain as the Slint property).
    pub caret: usize,
    /// 0-based line the caret sits on.
    pub caret_line: i32,
    /// 0-based line of the fixed end of the block, `NO_ANCHOR` when idle.
    pub anchor_line: i32,
    /// 0-based character column where the block starts (equal to column_end
    /// for a caret-style, zero-width column).
    pub column: i32,
    /// 0-based character column where the block ends, exclusive. Larger than
    /// column only for a rectangle dragged with the mouse.
    pub column_end: i32,
}

impl EditResult {
    fn idle(text: &str, caret: usize) -> Self {
        let (line, column) = line_col_of(text, caret);
        Self {
            text: text.to_string(),
            caret,
            caret_line: line as i32,
            anchor_line: NO_ANCHOR,
            column: column as i32,
            column_end: column as i32,
        }
    }
}

fn split_lines(text: &str) -> Vec<Vec<char>> {
    text.split('\n').map(|line| line.chars().collect()).collect()
}

fn join_lines(lines: &[Vec<char>]) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.extend(line.iter());
    }
    out
}

/// Byte offset of `(line, column)` inside `lines`; both are clamped.
fn byte_offset(lines: &[Vec<char>], line: usize, column: usize) -> usize {
    let line = line.min(lines.len().saturating_sub(1));
    let mut bytes = 0usize;
    for previous in &lines[..line] {
        bytes += previous.iter().map(|c| c.len_utf8()).sum::<usize>() + 1;
    }
    let column = column.min(lines[line].len());
    bytes
        + lines[line][..column]
            .iter()
            .map(|c| c.len_utf8())
            .sum::<usize>()
}

/// 0-based (line, character column) of a byte offset; a mid-character offset
/// resolves to the character it falls in, which never happens for real carets.
pub fn line_col_of(text: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(text.len());
    let mut line = 0usize;
    let mut column = 0usize;
    let mut seen = 0usize;
    for ch in text.chars() {
        if seen >= byte {
            break;
        }
        seen += ch.len_utf8();
        if ch == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn block_bounds(anchor: i32, caret_line: usize, last: usize) -> (usize, usize) {
    let anchor = (anchor.max(0) as usize).min(last);
    (anchor.min(caret_line), anchor.max(caret_line))
}

fn finish(lines: Vec<Vec<char>>, caret_line: usize, column: usize, anchor: i32) -> EditResult {
    finish_rect(lines, caret_line, column, column, anchor)
}

/// Same as finish, but the answer also carries the far end of a rectangle.
fn finish_rect(
    lines: Vec<Vec<char>>,
    caret_line: usize,
    column: usize,
    column_end: usize,
    anchor: i32,
) -> EditResult {
    let text = join_lines(&lines);
    let last = lines.len().saturating_sub(1);
    let caret_line = caret_line.min(last);
    let line_len = lines[caret_line].len();
    let column = column.min(line_len);
    let caret = byte_offset(&lines, caret_line, column);
    EditResult {
        text,
        caret,
        caret_line: caret_line as i32,
        anchor_line: if anchor == NO_ANCHOR {
            NO_ANCHOR
        } else {
            (anchor as usize).min(last) as i32
        },
        column: column as i32,
        column_end: column_end.min(line_len) as i32,
    }
}

/// A column-mode answer keeps the column it was given even when the line under
/// the caret is shorter: the block's carets must stay aligned, and the caret
/// byte offset clamps itself inside `byte_offset`.
fn finish_column(lines: Vec<Vec<char>>, caret_line: usize, column: usize, anchor: i32) -> EditResult {
    finish_column_preserving(lines, caret_line, column, column, anchor)
}

fn finish_column_preserving(
    lines: Vec<Vec<char>>,
    caret_line: usize,
    column: usize,
    column_end: usize,
    anchor: i32,
) -> EditResult {
    let text = join_lines(&lines);
    let last = lines.len().saturating_sub(1);
    let caret_line = caret_line.min(last);
    let caret = byte_offset(&lines, caret_line, column);
    EditResult {
        text,
        caret,
        caret_line: caret_line as i32,
        anchor_line: if anchor == NO_ANCHOR {
            NO_ANCHOR
        } else {
            (anchor as usize).min(last) as i32
        },
        column: column as i32,
        column_end: column_end as i32,
    }
}
/// Run `op` against a real rectangle: every line of the block loses the
/// characters in `column_start..column_end` (a typed character replaces them,
/// Backspace/Delete drop them, Enter turns the rest of the region into a line
/// break). The block then collapses to a zero-width column so the next
/// keystroke behaves like ordinary column editing.
fn apply_rect_op(
    mut lines: Vec<Vec<char>>,
    caret_line: usize,
    anchor: i32,
    column_start: usize,
    column_end: usize,
    op: i32,
    insert: &str,
) -> EditResult {
    let last = lines.len().saturating_sub(1);
    let (top, bottom) = block_bounds(anchor, caret_line, last);
    match op {
        OP_CANCEL => {
            let caret = byte_offset(&lines, caret_line, column_start);
            EditResult {
                text: join_lines(&lines),
                caret,
                caret_line: caret_line as i32,
                anchor_line: NO_ANCHOR,
                column: column_start as i32,
                column_end: column_start as i32,
            }
        }
        OP_INSERT => {
            let chars: Vec<char> = insert.chars().collect();
            for line in &mut lines[top..=bottom] {
                let end = column_end.min(line.len());
                let start = column_start.min(end);
                line.splice(start..end, chars.iter().copied());
            }
            let column = column_start + chars.len();
            finish_rect(lines, caret_line, column, column, anchor)
        }
        OP_BACKSPACE | OP_DELETE if column_end == column_start => {
            // A zero-width block is a caret on every line: Backspace eats the
            // character to its left, Delete the one under it. At a line edge the
            // block joins lines instead, the way the single-caret path does.
            if op == OP_BACKSPACE && column_start == 0 {
                // Everything the block covers collapses onto the line above it
                // (or onto the top of the document).
                let join_base = top.saturating_sub(1);
                let prefix: usize = lines[join_base..caret_line].iter().map(|l| l.len()).sum();
                let mut moved = 0usize;
                for index in (join_base + 1)..=bottom {
                    let line = lines.remove(index - moved);
                    lines[join_base].extend(line);
                    moved += 1;
                }
                return finish_rect(lines, join_base, prefix, prefix, join_base as i32);
            }
            if op == OP_DELETE && column_start >= lines[caret_line].len() {
                // Drop the newline after every line of the block, so the block
                // becomes a single line.
                for index in (top..=bottom).rev() {
                    if index + 1 < lines.len() {
                        let next = lines.remove(index + 1);
                        lines[index].extend(next);
                    }
                }
                return finish_rect(lines, top, column_start, column_start, anchor);
            }
            let (start, end) = if op == OP_BACKSPACE {
                (column_start.saturating_sub(1), column_start)
            } else {
                (column_start, column_start + 1)
            };
            for line in &mut lines[top..=bottom] {
                let end = end.min(line.len());
                let start = start.min(end);
                line.drain(start..end);
            }
            let column = if op == OP_BACKSPACE {
                column_start.saturating_sub(1)
            } else {
                column_start
            };
            finish_rect(lines, caret_line, column, column, anchor)
        }
        OP_BACKSPACE | OP_DELETE => {
            for line in &mut lines[top..=bottom] {
                let end = column_end.min(line.len());
                let start = column_start.min(end);
                line.drain(start..end);
            }
            finish_rect(lines, caret_line, column_start, column_start, anchor)
        }
        OP_NEWLINE => {
            // Descending order keeps the indices of the lines still to visit
            // valid while each split inserts a line after itself.
            for index in (top..=bottom).rev() {
                let end = column_end.min(lines[index].len());
                let start = column_start.min(end);
                let tail: Vec<char> = lines[index].split_off(end);
                lines[index].truncate(start);
                lines.insert(index + 1, tail);
            }
            let target = 2 * bottom + 1;
            finish_rect(lines, target, 0, 0, anchor)
        }
        OP_MOVE_LEFT => {
            // Arrow keys collapse the rectangle onto the edge they move to; a
            // zero-width block just steps one column left.
            let column = if column_end > column_start {
                column_start
            } else {
                column_start.saturating_sub(1)
            };
            finish_column(lines, caret_line, column, anchor)
        }
        OP_MOVE_RIGHT => {
            let column = if column_end > column_start {
                column_end
            } else {
                column_end + 1
            };
            finish_column(lines, caret_line, column, anchor)
        }
        OP_MOVE_DOWN | OP_SEL_DOWN | OP_MOVE_UP | OP_SEL_UP => {
            // Grow or shrink the block one line at a time. The column block
            // keeps its column, so the carets stay aligned.
            let down = matches!(op, OP_MOVE_DOWN | OP_SEL_DOWN);
            let target = if down {
                (caret_line + 1).min(last)
            } else {
                caret_line.saturating_sub(1)
            };
            finish_column_preserving(lines, target, column_start, column_end, anchor)
        }
        _ => finish_rect(lines, caret_line, column_start, column_start, anchor),
    }
}

/// Cut one character rectangle out of `text` and return it as clipboard text.
/// `top_line`/`bottom_line` and `start_col`/`end_col` are the corners in either
/// order. Lines outside the text are ignored and a line that ends before
/// `end_col` contributes only what it has, so no padding spaces are invented.
pub fn extract_rect(
    text: &str,
    top_line: i32,
    bottom_line: i32,
    start_col: i32,
    end_col: i32,
) -> String {
    // Same normalization as `apply_rect`: a pasted value can carry CR.
    let normalized: String;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        text
    };
    let lines = split_lines(text);
    let last = lines.len() as i32 - 1;
    let (top, bottom) = if top_line <= bottom_line {
        (top_line, bottom_line)
    } else {
        (bottom_line, top_line)
    };
    let (first_col, last_col) = if start_col <= end_col {
        (start_col, end_col)
    } else {
        (end_col, start_col)
    };
    let top = top.clamp(0, last) as usize;
    let bottom = bottom.clamp(0, last) as usize;
    let start = first_col.max(0) as usize;
    let end = last_col.max(0) as usize;
    let mut out = String::new();
    for (index, line) in lines[top..=bottom].iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let from = start.min(line.len());
        let to = end.min(line.len()).max(from);
        out.extend(&line[from..to]);
    }
    out
}

/// Apply `op` to `text`. `caret` is a byte offset, `anchor_line` the fixed end
/// of the column block (`NO_ANCHOR` when column mode is off) and `insert` the
/// typed text for `OP_INSERT`. The column comes from the caret: this is the
/// keyboard-only entry point (a zero-width column block). Only the tests call
/// it directly; the UI always goes through `apply_rect`.
#[allow(dead_code)]
pub fn apply(text: &str, caret: usize, anchor_line: i32, op: i32, insert: &str) -> EditResult {
    apply_rect(text, caret, anchor_line, -1, -1, op, insert)
}

/// Rectangle-aware edit: `start_col`/`end_col` are the character columns of the
/// region dragged with the mouse, in either order. A negative value means
/// "take the column from the caret", which is what every keyboard chord does.
pub fn apply_rect(
    text: &str,
    caret: usize,
    anchor_line: i32,
    start_col: i32,
    end_col: i32,
    op: i32,
    insert: &str,
) -> EditResult {
    // The editor never produces CR, but a pasted value can carry it; normalize
    // and keep the caret on the same logical position.
    let normalized: String;
    let (text, caret) = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let (line, column) = line_col_of(text, caret);
        let lines = split_lines(&normalized);
        (normalized.as_str(), byte_offset(&lines, line, column))
    } else {
        (text, caret)
    };

    let mut lines = split_lines(text);
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    let last = lines.len() - 1;
    let (mut caret_line, mut column) = line_col_of(text, caret);
    caret_line = caret_line.min(last);
    column = column.min(lines[caret_line].len());
    let mut anchor = anchor_line.clamp(NO_ANCHOR, last as i32);
    // Explicit columns make a rectangle; otherwise the block is the caret's
    // own column and every edit is zero-width.
    let (column_start, column_end) = if start_col < 0 || end_col < 0 {
        (column, column)
    } else {
        let a = start_col.max(0) as usize;
        let b = end_col.max(0) as usize;
        (a.min(b), a.max(b))
    };

    // The mouse asks where the caret itself is, so a chord click can start a
    // rectangle from it.
    if op == OP_LOCATE {
        return EditResult::idle(text, caret.min(text.len()));
    }
    // The mouse asks where a (line, column) pair lives in byte offsets before
    // it moves the caret itself.
    if op == OP_PLACE {
        let line = anchor.clamp(0, last as i32) as usize;
        let at = column_start.min(lines[line].len());
        return EditResult {
            text: text.to_string(),
            caret: byte_offset(&lines, line, at),
            caret_line: line as i32,
            anchor_line: NO_ANCHOR,
            column: at as i32,
            column_end: at as i32,
        };
    }
    // A zero-width block over several lines is still a block: typing inserts
    // at that column on every one of them, which is what the carets the editor
    // draws promise.
    let block_is_multiline = anchor != NO_ANCHOR && anchor != caret_line as i32;
    if anchor != NO_ANCHOR && (column_end > column_start || block_is_multiline) {
        return apply_rect_op(
            lines,
            caret_line,
            anchor,
            column_start,
            column_end,
            op,
            insert,
        );
    }
    let mut column = column_start;

    match op {
        OP_CANCEL => {
            return EditResult {
                text: text.to_string(),
                caret: caret.min(text.len()),
                caret_line: caret_line as i32,
                anchor_line: NO_ANCHOR,
                column: column as i32,
                column_end: column as i32,
            };
        }
        OP_SEL_DOWN | OP_MOVE_DOWN | OP_SEL_UP | OP_MOVE_UP => {
            let down = matches!(op, OP_SEL_DOWN | OP_MOVE_DOWN);
            let target = if down {
                (caret_line + 1).min(last)
            } else {
                caret_line.saturating_sub(1)
            };
            if target == caret_line {
                return finish(lines, caret_line, column, anchor);
            }
            // The shift variants may start a block; the plain ones only move an
            // already active caret end.
            if anchor == NO_ANCHOR {
                if matches!(op, OP_SEL_DOWN | OP_SEL_UP) {
                    anchor = caret_line as i32;
                } else {
                    return finish(lines, caret_line, column, anchor);
                }
            }
            caret_line = target;
        }
        OP_NEWLINE => {
            if anchor == NO_ANCHOR {
                let at = column.min(lines[caret_line].len());
                let tail: Vec<char> = lines[caret_line].split_off(at);
                lines.insert(caret_line + 1, tail);
                caret_line += 1;
                column = 0;
            } else {
                let (top, bottom) = block_bounds(anchor, caret_line, last);
                // Descending order keeps the indices of the lines still to
                // visit valid while each split inserts a line after itself.
                for index in (top..=bottom).rev() {
                    let at = column.min(lines[index].len());
                    let tail: Vec<char> = lines[index].split_off(at);
                    lines.insert(index + 1, tail);
                }
                caret_line = 2 * bottom + 1;
                column = 0;
            }
        }
        OP_MOVE_LEFT | OP_MOVE_RIGHT => {
            if anchor == NO_ANCHOR {
                return EditResult::idle(text, caret.min(text.len()));
            }
            let len = lines[caret_line].len();
            column = if op == OP_MOVE_LEFT {
                column.saturating_sub(1)
            } else {
                (column + 1).min(len)
            };
        }
        OP_INSERT => {
            if anchor == NO_ANCHOR {
                return EditResult::idle(text, caret.min(text.len()));
            }
            let chars: Vec<char> = insert.chars().collect();
            if chars.is_empty() {
                return finish(lines, caret_line, column, anchor);
            }
            let (top, bottom) = block_bounds(anchor, caret_line, last);
            let inserted_at = column.min(lines[caret_line].len());
            for line in &mut lines[top..=bottom] {
                let at = column.min(line.len());
                line.splice(at..at, chars.iter().copied());
            }
            column = inserted_at + chars.len();
        }
        OP_BACKSPACE => {
            if anchor == NO_ANCHOR {
                return EditResult::idle(text, caret.min(text.len()));
            }
            let (top, bottom) = block_bounds(anchor, caret_line, last);
            if column == 0 {
                // Deleting the first character of every line deletes the
                // newlines instead: the whole block collapses into one line.
                let join_column: usize = (top..caret_line).map(|i| lines[i].len()).sum();
                if bottom > top {
                    let mut merged: Vec<char> = Vec::new();
                    for index in top..=bottom {
                        merged.extend(lines[index].iter().copied());
                    }
                    let merged_len = merged.len();
                    lines.splice(top..=bottom, [merged]);
                    caret_line = top;
                    column = join_column.min(merged_len);
                }
            } else {
                for line in &mut lines[top..=bottom] {
                    if column <= line.len() {
                        line.remove(column - 1);
                    }
                }
                column -= 1;
            }
        }
        OP_DELETE => {
            if anchor == NO_ANCHOR {
                return EditResult::idle(text, caret.min(text.len()));
            }
            let (top, bottom) = block_bounds(anchor, caret_line, last);
            // Bottom-up: a join removes the line *below* the cursor, so the
            // indices still to be visited stay valid.
            let mut shift = 0usize;
            let mut caret_absorbed = false;
            for index in (top..=bottom).rev() {
                if column < lines[index].len() {
                    lines[index].remove(column);
                } else if index + 1 < lines.len() {
                    // At EOL the newline goes away: the next line is pulled up.
                    let next = lines.remove(index + 1);
                    lines[index].extend(next);
                    if index + 1 < caret_line {
                        shift += 1;
                    } else if index + 1 == caret_line {
                        caret_absorbed = true;
                    }
                }
            }
            caret_line = caret_line.saturating_sub(shift);
            if caret_absorbed {
                caret_line = caret_line.saturating_sub(1);
            }
        }
        _ => return EditResult::idle(text, caret.min(text.len())),
    }

    finish(lines, caret_line, column, anchor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret_of(text: &str, line: usize, column: usize) -> usize {
        byte_offset(&split_lines(text), line, column)
    }

    #[test]
    fn line_col_round_trips_with_unicode() {
        let text = "ab\n中文字\nx";
        assert_eq!(line_col_of(text, 0), (0, 0));
        assert_eq!(line_col_of(text, 3), (1, 0));
        assert_eq!(line_col_of(text, 6), (1, 1)); // after the 中 (3 bytes)
        assert_eq!(caret_of(text, 1, 1), 6);
        assert_eq!(caret_of(text, 2, 1), text.len());
    }

    #[test]
    fn shift_down_activates_then_extends() {
        let text = "aa\nbb\ncc";
        let first = apply(text, caret_of(text, 0, 0), NO_ANCHOR, OP_SEL_DOWN, "");
        assert_eq!(first.anchor_line, 0);
        assert_eq!(first.caret_line, 1);
        assert_eq!(first.text, text);

        let second = apply(text, first.caret, first.anchor_line, OP_SEL_DOWN, "");
        assert_eq!(second.caret_line, 2);
        assert_eq!(second.anchor_line, 0);

        let clamped = apply(text, second.caret, second.anchor_line, OP_SEL_DOWN, "");
        assert_eq!(clamped.caret_line, 2);
    }

    #[test]
    fn shift_up_on_first_line_stays_idle() {
        let text = "aa\nbb";
        let result = apply(text, 0, NO_ANCHOR, OP_SEL_UP, "");
        assert_eq!(result.anchor_line, NO_ANCHOR);
        assert_eq!(result.caret_line, 0);
    }

    #[test]
    fn insert_hits_every_line_of_the_block() {
        let text = "one\ntwo\nthree\nfour";
        let caret = caret_of(text, 1, 1);
        let result = apply(text, caret, 0, OP_INSERT, "-");
        // Lines 0..=1 only (the block is anchor..caret).
        assert_eq!(result.text, "o-ne\nt-wo\nthree\nfour");
        assert_eq!(line_col_of(&result.text, result.caret), (1, 2));
        assert_eq!(result.anchor_line, 0);
    }

    #[test]
    fn insert_past_a_short_line_appends_at_its_end() {
        let text = "abcd\nx\nabcd\n";
        let caret = caret_of(text, 1, 1);
        let result = apply(text, caret, 0, OP_INSERT, "Z");
        // Line 1 only has one char: the Z lands after it.
        assert_eq!(result.text, "aZbcd\nxZ\nabcd\n");
    }

    #[test]
    fn backspace_at_column_zero_joins_the_block() {
        let text = "abc\ndef\nghi";
        let caret = caret_of(text, 2, 0);
        let result = apply(text, caret, 0, OP_BACKSPACE, "");
        assert_eq!(result.text, "abcdefghi");
        // Caret sits where "ghi" used to start.
        assert_eq!(line_col_of(&result.text, result.caret), (0, 6));
        assert_eq!(result.anchor_line, 0);
    }

    #[test]
    fn backspace_deletes_before_the_column_on_every_line() {
        let text = "abcd\nab\nabcd";
        let caret = caret_of(text, 1, 2);
        let result = apply(text, caret, 0, OP_BACKSPACE, "");
        // Line 1 is only two chars: its b goes, the others lose their second.
        // Line 2 is outside the block and stays untouched.
        assert_eq!(result.text, "acd\na\nabcd");
        assert_eq!(line_col_of(&result.text, result.caret), (1, 1));
    }

    #[test]
    fn delete_removes_the_first_character_of_each_line() {
        let text = "abc\ndef\ngh";
        let caret = caret_of(text, 0, 0);
        let result = apply(text, caret, 2, OP_DELETE, "");
        assert_eq!(result.text, "bc\nef\nh");
    }

    #[test]
    fn delete_at_eol_joins_the_block() {
        let text = "ab\ncd\nef";
        let caret = caret_of(text, 1, 2);
        let result = apply(text, caret, 0, OP_DELETE, "");
        // Every line of the block loses its newline: the block becomes one line.
        assert_eq!(result.text, "abcdef");
        assert_eq!(result.caret_line, 0);
    }

    #[test]
    fn arrows_move_the_column_while_active() {
        let text = "abc\ndef";
        let caret = caret_of(text, 0, 1);
        let right = apply(text, caret, 0, OP_MOVE_RIGHT, "");
        assert_eq!(right.column, 2);
        let left = apply(text, right.caret, 0, OP_MOVE_LEFT, "");
        assert_eq!(left.column, 1);
        // Clamped at the line start.
        let start = apply(text, caret_of(text, 0, 0), 0, OP_MOVE_LEFT, "");
        assert_eq!(start.column, 0);
    }

    #[test]
    fn cancel_leaves_the_text_alone() {
        let text = "abc\ndef";
        let caret = caret_of(text, 1, 2);
        let result = apply(text, caret, 0, OP_CANCEL, "");
        assert_eq!(result.text, text);
        assert_eq!(result.anchor_line, NO_ANCHOR);
        assert_eq!(result.caret_line, 1);
    }

    #[test]
    fn crlf_is_normalized_and_the_caret_still_lands() {
        let text = "ab\r\ncd";
        let result = apply(text, 3, NO_ANCHOR, OP_SEL_DOWN, "");
        assert_eq!(result.text, "ab\ncd");
        assert_eq!(result.caret_line, 1);
    }

    #[test]
    fn newline_splits_the_caret_line() {
        let text = "abcdef";
        let caret = caret_of(text, 0, 3);
        let result = apply(text, caret, NO_ANCHOR, OP_NEWLINE, "");
        assert_eq!(result.text, "abc\ndef");
        assert_eq!(line_col_of(&result.text, result.caret), (1, 0));
        assert_eq!(result.anchor_line, NO_ANCHOR);
    }

    #[test]
    fn newline_in_column_mode_splits_every_line() {
        let text = "ab\ncd";
        let caret = caret_of(text, 1, 1);
        let result = apply(text, caret, 0, OP_NEWLINE, "");
        assert_eq!(result.text, "a\nb\nc\nd");
        // The caret ends on the break inserted for the last line of the block.
        assert_eq!(line_col_of(&result.text, result.caret), (3, 0));
        assert_eq!(result.anchor_line, 0);
    }

    #[test]
    fn unicode_columns_count_characters_not_bytes() {
        let text = "中文\n日本語";
        // Caret after the second char of line 0 → column 2 on both lines.
        let caret = caret_of(text, 0, 2);
        let result = apply(text, caret, 1, OP_INSERT, "!");
        assert_eq!(result.text, "中文!\n日本!語");
        assert_eq!(line_col_of(&result.text, result.caret), (0, 3));
    }

    #[test]
    fn rectangle_insert_replaces_the_region_on_every_line() {
        let text = "abcd\nefgh";
        let caret = caret_of(text, 0, 1);
        let result = apply_rect(text, caret, 1, 1, 3, OP_INSERT, "X");
        // "bc" and "fg" both become a single X.
        assert_eq!(result.text, "aXd\neXh");
        assert_eq!(line_col_of(&result.text, result.caret), (0, 2));
        // The block collapses onto the caret column.
        assert_eq!(result.column, 2);
        assert_eq!(result.column_end, 2);
    }

    #[test]
    fn rectangle_columns_work_in_either_direction() {
        let text = "abcd\nefgh";
        let caret = caret_of(text, 0, 1);
        let forward = apply_rect(text, caret, 1, 1, 3, OP_INSERT, "X");
        let backward = apply_rect(text, caret, 1, 3, 1, OP_INSERT, "X");
        assert_eq!(forward.text, backward.text);
        assert_eq!(forward.caret, backward.caret);
    }

    #[test]
    fn rectangle_delete_removes_the_region_on_every_line() {
        let text = "abcd\nefgh";
        let caret = caret_of(text, 0, 1);
        let result = apply_rect(text, caret, 1, 1, 3, OP_BACKSPACE, "");
        assert_eq!(result.text, "ad\neh");
        // A short line only loses the characters the region actually covers:
        // column 1 of "x" is past its end, so the line stays as it is.
        let short = apply_rect("abcd\nx", caret_of("abcd\nx", 0, 1), 1, 1, 3, OP_DELETE, "");
        assert_eq!(short.text, "ad\nx");
    }

    #[test]
    fn rectangle_newline_turns_the_region_into_line_breaks() {
        let text = "abcd\nefgh";
        let caret = caret_of(text, 0, 1);
        let result = apply_rect(text, caret, 1, 1, 3, OP_NEWLINE, "");
        // Each line of the block keeps its left half, and its right half moves
        // to the line break inserted after it.
        assert_eq!(result.text, "a\nd\ne\nh");
        assert_eq!(line_col_of(&result.text, result.caret), (3, 0));
    }

    #[test]
    fn place_only_moves_the_caret() {
        let text = "ab\ncd\nef";
        let result = apply_rect(text, 0, 2, 2, 2, OP_PLACE, "");
        assert_eq!(result.text, text);
        assert_eq!(result.caret, text.len());
        assert_eq!(result.caret_line, 2);
        assert_eq!(result.column, 2);
        assert_eq!(result.anchor_line, NO_ANCHOR);
    }

    #[test]
    fn locate_reports_the_caret_without_touching_the_text() {
        let text = "ab
cd
ef";
        let caret = caret_of(text, 1, 1);
        let result = apply_rect(text, caret, 0, 0, 0, OP_LOCATE, "");
        assert_eq!(result.text, text);
        assert_eq!(result.caret, caret);
        assert_eq!(result.caret_line, 1);
        assert_eq!(result.column, 1);
        assert_eq!(result.anchor_line, NO_ANCHOR);
    }

    #[test]
    fn locate_clamps_a_caret_past_the_end() {
        let text = "ab";
        let result = apply_rect(text, 99, 0, 0, 0, OP_LOCATE, "");
        assert_eq!(result.caret, 2);
        assert_eq!(result.caret_line, 0);
        assert_eq!(result.column, 2);
    }

    #[test]
    fn column_arrows_keep_the_column_on_a_short_line() {
        // The middle line is shorter than the block column: moving the block
        // must not snap it back to that line's end, or the carets drift.
        let text = "abcdef\nab\nuvwxyz";
        let caret = caret_of(text, 1, 2);
        let right = apply_rect(text, caret, 0, 4, 4, OP_MOVE_RIGHT, "");
        assert_eq!(right.column, 5);
        assert_eq!(right.column_end, 5);
        assert_eq!(right.anchor_line, 0);
        let left = apply_rect(text, right.caret, 0, 5, 5, OP_MOVE_LEFT, "");
        assert_eq!(left.column, 4);
        assert_eq!(left.text, text);
    }

    #[test]
    fn column_arrows_collapse_a_rectangle() {
        let text = "abcdef\nghijkl";
        let caret = caret_of(text, 1, 1);
        let left = apply_rect(text, caret, 0, 1, 4, OP_MOVE_LEFT, "");
        assert_eq!(left.column, 1);
        assert_eq!(left.column_end, 1);
        let right = apply_rect(text, caret, 0, 1, 4, OP_MOVE_RIGHT, "");
        assert_eq!(right.column, 4);
        assert_eq!(right.text, text);
    }

    #[test]
    fn extract_rect_takes_character_columns() {
        let text = "abcdef\nghijkl\nmnopqr";
        assert_eq!(extract_rect(text, 0, 2, 1, 4), "bcd\nhij\nnop");
        // The corners arrive in either order.
        assert_eq!(extract_rect(text, 2, 0, 4, 1), "bcd\nhij\nnop");
        assert_eq!(extract_rect(text, 1, 1, 2, 99), "ijkl");
    }

    #[test]
    fn extract_rect_truncates_short_lines_without_padding() {
        let text = "abcdef\nab\nabcdef";
        assert_eq!(extract_rect(text, 0, 2, 2, 5), "cde\n\ncde");
        assert_eq!(extract_rect(text, 1, 1, 3, 5), "");
    }

    #[test]
    fn extract_rect_ignores_lines_outside_the_text() {
        let text = "one\ntwo";
        assert_eq!(extract_rect(text, -5, 90, 0, 3), "one\ntwo");
        assert_eq!(extract_rect("", 0, 3, 0, 3), "");
    }

    #[test]
    fn extract_rect_normalizes_crlf() {
        let text = "abcdef\r\nghijkl";
        assert_eq!(extract_rect(text, 0, 1, 2, 4), "cd\nij");
    }
}

