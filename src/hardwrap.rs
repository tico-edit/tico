//! Automatic hard-wrapping of an overlong line while typing (`set
//! breaklonglines`, `-b`, `M-L`): a port of nano's `do_wrap` and the
//! `break_line` it relies on (`src/text.c`), plus the slice of
//! `do_enter` it ends with.
//!
//! Unlike `justify`, this works in display columns (tabs expanded, wide
//! and control characters counting two), as nano does: the decision is
//! made on every keystroke, so a tab-indented line has to wrap where the
//! user sees the fill column, not where a character count would put it.

use crate::buffer::char_width;
use crate::justify::{begpar, indent_length, inpar, quote_length};
use regex::Regex;

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// What a hard wrap needs to know beyond the text itself.
pub struct WrapParams<'a> {
    pub wrap_at: usize,
    pub tabsize: usize,
    pub quote_re: Option<&'a Regex>,
    pub autoindent: bool,
    pub trimblanks: bool,
}

/// The result of wrapping line `idx`: lines `idx..idx + replaced` give
/// way to `lines` (always two -- the shortened line and the overflow
/// line), and the cursor goes to line `idx + cursor.0`, column
/// `cursor.1`. The overflow line becomes the new spillage line.
#[derive(Debug, PartialEq, Eq)]
pub struct Wrapped {
    pub replaced: usize,
    pub lines: [Vec<char>; 2],
    pub cursor: (usize, usize),
}

fn advance(column: usize, c: char, tabsize: usize) -> usize {
    if c == '\t' {
        column + tabsize - column % tabsize
    } else {
        column + char_width(c)
    }
}

/// nano's `break_line` (without its help-text cases): the index of the
/// last blank in `text` at or before display column `goal + 1` -- or,
/// when there is none, of the first blank after it -- moved on to the
/// last blank of the run it starts. `Some(text.len())` when the whole
/// text fits within `goal`; `None` when it doesn't and has no blank to
/// break at. Columns are counted from the start of `text`.
fn break_line(text: &[char], goal: isize, tabsize: usize) -> Option<usize> {
    let mut i = 0;
    let mut column = 0;
    // Never break in the leading whitespace.
    while i < text.len() && is_blank(text[i]) {
        column = advance(column, text[i], tabsize);
        i += 1;
    }
    let mut lastblank = None;
    while i < text.len() && column as isize <= goal {
        if is_blank(text[i]) {
            lastblank = Some(i);
        }
        column = advance(column, text[i], tabsize);
        i += 1;
    }
    if column as isize <= goal {
        return Some(i);
    }
    let mut lastblank = match lastblank {
        Some(b) => b,
        None => loop {
            if i >= text.len() {
                return None;
            }
            if is_blank(text[i]) {
                break i;
            }
            i += 1;
        },
    };
    while lastblank + 1 < text.len() && is_blank(text[lastblank + 1]) {
        lastblank += 1;
    }
    Some(lastblank)
}

/// Where `line` would be broken (the index of the first character to
/// move to the next line), with its quote and quote-plus-indent lengths;
/// `None` when it needs no wrapping or can't be wrapped.
fn wrap_location(line: &[char], p: &WrapParams) -> Option<(usize, usize, usize)> {
    let quot_len = quote_length(line, p.quote_re);
    let lead_len = quot_len + indent_length(&line[quot_len..]);
    let lead_width = line[..lead_len]
        .iter()
        .fold(0, |col, &c| advance(col, c, p.tabsize));
    let goal = p.wrap_at as isize - lead_width as isize;
    let blank = break_line(&line[lead_len..], goal, p.tabsize)?;
    if lead_len + blank == line.len() {
        return None;
    }
    // Step past the blank itself; nothing to wrap when that's the end.
    let wrap_loc = lead_len + blank + 1;
    (wrap_loc < line.len()).then_some((wrap_loc, quot_len, lead_len))
}

/// Whether `line` is overlong and has a blank to break it at -- a cheap
/// test on the one line, so the caller only gathers the surrounding
/// lines `hard_wrap` needs when there's actually something to do.
pub fn needs_wrap(line: &[char], p: &WrapParams) -> bool {
    wrap_location(line, p).is_some()
}

/// nano's `do_wrap` on line `idx` of `lines` with the cursor at column
/// `cursor_x`: break the line at the last blank that keeps it within
/// `wrap_at`, carrying the rest over to a new line -- or, when the line
/// below is the overflow line from the previous wrap (`spillage_is_next`)
/// and the rest fits in front of it, prepending it to that line instead,
/// so that typing in the middle of a paragraph pushes words along rather
/// than leaving a trail of short lines.
pub fn hard_wrap(
    lines: &[Vec<char>],
    idx: usize,
    cursor_x: usize,
    spillage_is_next: bool,
    p: &WrapParams,
) -> Option<Wrapped> {
    let line = &lines[idx];
    let (wrap_loc, quot_len, lead_len) = wrap_location(line, p)?;
    // nano switches autoindent off while wrapping a quoted line: the
    // quote prefix is copied over instead.
    let autoindent = p.autoindent && quot_len == 0;

    let mut cur = line.clone();
    let mut replaced = 1;
    let remainder = &line[wrap_loc..];
    // nano compares this byte length against a display width.
    let rest_length: usize = remainder.iter().map(|c| c.len_utf8()).sum();
    let next = lines.get(idx + 1);
    if let Some(next) = next.filter(|_| spillage_is_next)
        && rest_length + next.iter().fold(0, |col, &c| advance(col, c, p.tabsize)) <= p.wrap_at
    {
        let mut cx = cur.len();
        if !remainder.last().is_some_and(|&c| is_blank(c)) {
            cur.push(' ');
            cx += 1;
        }
        cur.extend_from_slice(next);
        replaced = 2;
        // The overflow line's own copy of the lead goes.
        if cur.len() >= cx + lead_len && cur[..lead_len] == cur[cx..cx + lead_len] {
            cur.drain(cx..cx + lead_len);
        }
        let blanks = cur[cx..].iter().take_while(|&&c| is_blank(c)).count();
        cur.drain(cx..cx + blanks);
    }

    let mut cx = wrap_loc;
    if p.trimblanks {
        // Snip the blanks that would trail the shortened line -- but not
        // one the user just typed in front of the cursor.
        let typed_x = cursor_x.saturating_sub(1);
        let mut rear_x = wrap_loc - 1;
        while (rear_x != typed_x || cursor_x >= wrap_loc) && is_blank(cur[rear_x]) {
            cx = rear_x;
            cur.remove(rear_x);
            rear_x = rear_x.saturating_sub(1);
        }
    }

    // The split itself: nano's `do_enter`, which under autoindent takes
    // its indentation from the line below when that continues the same
    // paragraph (so a hanging indent is kept), else from this line.
    let (extra, allblanks, indent) = if autoindent {
        let mut work: Vec<Vec<char>> = Vec::with_capacity(idx + 2);
        work.extend_from_slice(&lines[..idx]);
        work.push(cur.clone());
        if let Some(below) = lines.get(idx + replaced) {
            work.push(below.clone());
        }
        let sample = match work.get(idx + 1) {
            Some(below)
                if inpar(below, p.quote_re) && !begpar(&work, idx + 1, p.quote_re, false) =>
            {
                below
            }
            _ => &cur,
        };
        let mut extra = indent_length(sample);
        let mut allblanks = false;
        if extra > cx {
            extra = cx;
        } else if extra == cx {
            allblanks = indent_length(&cur) == extra;
        }
        (extra, allblanks, sample[..extra].to_vec())
    } else {
        (0, false, Vec::new())
    };
    let mut overflow = indent;
    overflow.extend_from_slice(&cur[cx..]);
    cur.truncate(if allblanks { 0 } else { cx });
    let mut col = extra;
    if quot_len > 0 {
        let mut quoted = cur[..lead_len].to_vec();
        quoted.extend(overflow);
        overflow = quoted;
        col += lead_len;
    }

    let cursor = if cursor_x < wrap_loc {
        (0, cursor_x.min(cur.len()))
    } else {
        (1, col + cursor_x - wrap_loc)
    };
    Some(Wrapped {
        replaced,
        lines: [cur, overflow],
        cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<Vec<char>> {
        s.split('\n').map(|l| l.chars().collect()).collect()
    }

    fn params(wrap_at: usize) -> WrapParams<'static> {
        WrapParams {
            wrap_at,
            tabsize: 8,
            quote_re: None,
            autoindent: false,
            trimblanks: false,
        }
    }

    fn strings(w: &Wrapped) -> (String, String) {
        (w.lines[0].iter().collect(), w.lines[1].iter().collect())
    }

    #[test]
    fn a_line_within_the_width_or_without_blanks_is_left_alone() {
        let p = params(10);
        assert!(!needs_wrap(&lines("one two")[0], &p));
        assert!(!needs_wrap(&lines("abcdefghijklmnop")[0], &p));
        // Only the blank at the very end is past the width: nothing to move.
        assert!(!needs_wrap(&lines("abcdefghijklmnop ")[0], &p));
    }

    #[test]
    fn typing_past_the_width_moves_the_last_word_down() {
        let l = lines("one two three");
        let w = hard_wrap(&l, 0, 13, false, &params(10)).unwrap();
        assert_eq!(strings(&w), ("one two ".into(), "three".into()));
        assert_eq!(w.replaced, 1);
        assert_eq!(w.cursor, (1, 5));
    }

    #[test]
    fn a_wrap_before_the_cursor_keeps_it_on_the_shortened_line() {
        let l = lines("one two three");
        let w = hard_wrap(&l, 0, 2, false, &params(10)).unwrap();
        assert_eq!(w.cursor, (0, 2));
    }

    #[test]
    fn the_rest_is_prepended_to_the_previous_overflow_line_when_it_fits() {
        let l = lines("one two three\nfour");
        let w = hard_wrap(&l, 0, 2, true, &params(10)).unwrap();
        assert_eq!(strings(&w), ("one two ".into(), "three four".into()));
        assert_eq!(w.replaced, 2);
        // ... but not when the line below isn't the overflow line.
        let w = hard_wrap(&l, 0, 2, false, &params(10)).unwrap();
        assert_eq!(strings(&w), ("one two ".into(), "three".into()));
        assert_eq!(w.replaced, 1);
    }

    #[test]
    fn trimblanks_snips_the_blanks_before_the_break() {
        let l = lines("one two   three");
        let mut p = params(10);
        p.trimblanks = true;
        let w = hard_wrap(&l, 0, 15, false, &p).unwrap();
        assert_eq!(strings(&w), ("one two".into(), "three".into()));
    }

    #[test]
    fn autoindent_carries_the_indent_over() {
        let l = lines("    one two three");
        let mut p = params(14);
        p.autoindent = true;
        let w = hard_wrap(&l, 0, 17, false, &p).unwrap();
        assert_eq!(strings(&w), ("    one two ".into(), "    three".into()));
        assert_eq!(w.cursor, (1, 9));
    }

    #[test]
    fn a_quoted_line_keeps_its_quote_on_the_overflow_line() {
        let re = Regex::new(r"^([ \t]*([!#%:;>|}]|//))+").unwrap();
        let l = lines("> one two three");
        let mut p = params(10);
        p.quote_re = Some(&re);
        let w = hard_wrap(&l, 0, 15, false, &p).unwrap();
        assert_eq!(strings(&w), ("> one two ".into(), "> three".into()));
        assert_eq!(w.cursor, (1, 7));
    }

    #[test]
    fn tabs_count_as_display_columns() {
        // A tab indent takes eight columns, leaving only two of ten.
        let l = lines("\tab cd");
        assert!(needs_wrap(&l[0], &params(10)));
        assert!(!needs_wrap(&lines("  ab cd")[0], &params(10)));
    }
}
