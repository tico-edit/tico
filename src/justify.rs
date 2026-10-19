//! Paragraph reflow for `^J` Justify / `M-J` Full Justify: nano's
//! quote/indent-aware "fill paragraph", ported from `src/text.c`'s
//! `quote_length`/`indent_length`/`begpar`/`inpar`/`find_paragraph`/
//! `concat_paragraph`/`squeeze`/`break_line`/`rewrap_paragraph`/
//! `justify_paragraph`.
//!
//! Everything here works in character counts rather than nano's own
//! tab-aware display "wideness" (`breadth`/`xplustabs`), a deliberate
//! simplification: prose paragraphs are overwhelmingly made of plain
//! characters and space-indentation, so this matches nano exactly in the
//! common case and only drifts on lines that mix tabs into running text.

use regex::Regex;

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The column to wrap at, from `set fill`/`-r` and the screen width --
/// matches nano's own `wrap_at` computation in `src/nano.c`.
pub fn wrap_at(fill: i32, screen_cols: usize) -> usize {
    let cols = screen_cols as i64;
    let fill = fill as i64;
    if cols + fill <= 0 {
        0
    } else if fill <= 0 {
        (cols + fill) as usize
    } else {
        fill as usize
    }
}

/// nano's `quote_length`: the length (in chars) of the leading substring
/// of `line` matched by `quote_re`, when that match starts at position 0.
/// `None` (an unparseable `quotestr`) means "no quoting recognized".
pub(crate) fn quote_length(line: &[char], quote_re: Option<&Regex>) -> usize {
    let Some(re) = quote_re else {
        return 0;
    };
    let s: String = line.iter().collect();
    match re.find(&s) {
        Some(m) if m.start() == 0 => s[..m.end()].chars().count(),
        _ => 0,
    }
}

/// nano's `indent_length`: the leading run of blanks.
pub(crate) fn indent_length(line: &[char]) -> usize {
    line.iter().take_while(|&&c| is_blank(c)).count()
}

/// nano's `inpar`: whether `line` has any text beyond its quote+indent.
pub(crate) fn inpar(line: &[char], quote_re: Option<&Regex>) -> bool {
    let q = quote_length(line, quote_re);
    let i = indent_length(&line[q..]);
    line.len() > q + i
}

/// nano's `begpar`: whether `lines[idx]` begins a new paragraph.
/// `bookstyle` is `set bookstyle` in effect -- nano honors it only while
/// `autoindent` is off, which the caller folds in.
pub(crate) fn begpar(
    lines: &[Vec<char>],
    idx: usize,
    quote_re: Option<&Regex>,
    bookstyle: bool,
) -> bool {
    if idx == 0 {
        return true;
    }
    let line = &lines[idx];
    let quot_len = quote_length(line, quote_re);
    let indent_len = indent_length(&line[quot_len..]);
    if line.len() == quot_len + indent_len {
        return false;
    }
    // `set bookstyle`: a line starting with whitespace (before any quote
    // prefix, so an indented line under a `>` doesn't count) begins one.
    if bookstyle && line.first().is_some_and(|&c| is_blank(c)) {
        return true;
    }
    let prev = &lines[idx - 1];
    let prev_quot_len = quote_length(prev, quote_re);
    if quot_len != prev_quot_len || line[..quot_len] != prev[..quot_len] {
        return true;
    }
    let prev_indent_len = indent_length(&prev[quot_len..]);
    if prev.len() == quot_len + prev_indent_len {
        return true;
    }
    if indent_len == prev_indent_len {
        return false;
    }
    !begpar(lines, idx - 1, quote_re, bookstyle)
}

/// nano's `do_para_begin`: step back to the first line of the paragraph
/// that contains (or immediately precedes) `from`.
pub fn para_begin(
    lines: &[Vec<char>],
    from: usize,
    quote_re: Option<&Regex>,
    bookstyle: bool,
) -> usize {
    let mut idx = from.saturating_sub(1);
    while idx > 0 && !begpar(lines, idx, quote_re, bookstyle) {
        idx -= 1;
    }
    idx
}

/// nano's `do_para_end`: step forward to the last line of the first
/// paragraph found at or after `from` (the last line of the buffer when
/// there is none).
pub fn para_end(
    lines: &[Vec<char>],
    from: usize,
    quote_re: Option<&Regex>,
    bookstyle: bool,
) -> usize {
    let last = lines.len().saturating_sub(1);
    let mut idx = from.min(last);
    while idx < last && !inpar(&lines[idx], quote_re) {
        idx += 1;
    }
    while idx < last
        && inpar(&lines[idx + 1], quote_re)
        && !begpar(lines, idx + 1, quote_re, bookstyle)
    {
        idx += 1;
    }
    idx
}

/// Whether `lines[from]` sits inside a paragraph without being its first
/// line -- nano's own gate for whether `^J` should first back up to
/// `para_begin` before searching forward.
pub fn in_mid_paragraph(
    lines: &[Vec<char>],
    from: usize,
    quote_re: Option<&Regex>,
    bookstyle: bool,
) -> bool {
    inpar(&lines[from], quote_re) && !begpar(lines, from, quote_re, bookstyle)
}

/// nano's `find_paragraph`: from `start` onward, the first paragraph found
/// as (first line index, line count), or `None` if there isn't one.
pub fn find_paragraph(
    lines: &[Vec<char>],
    mut start: usize,
    quote_re: Option<&Regex>,
    bookstyle: bool,
) -> Option<(usize, usize)> {
    while start < lines.len() && !inpar(&lines[start], quote_re) {
        start += 1;
    }
    if start >= lines.len() {
        return None;
    }
    let mut end = start;
    while end + 1 < lines.len()
        && inpar(&lines[end + 1], quote_re)
        && !begpar(lines, end + 1, quote_re, bookstyle)
    {
        end += 1;
    }
    Some((start, end - start + 1))
}

/// nano's `concat_paragraph` followed by `squeeze`: join `count` lines
/// starting at `start` into one (stripping each line-after-the-first's
/// own lead), then normalize inner whitespace and trim trailing blanks --
/// everything up to the *first* line's own lead is left untouched.
fn concat_and_squeeze(
    lines: &[Vec<char>],
    start: usize,
    count: usize,
    quote_re: Option<&Regex>,
    punct: &str,
    brackets: &str,
) -> Vec<char> {
    let mut text = lines[start].clone();
    for next in &lines[start + 1..start + count] {
        let next_quot_len = quote_length(next, quote_re);
        let next_lead_len = next_quot_len + indent_length(&next[next_quot_len..]);
        if !text.is_empty() && *text.last().unwrap() != ' ' {
            text.push(' ');
        }
        text.extend_from_slice(&next[next_lead_len..]);
    }

    let first_quot_len = quote_length(&text, quote_re);
    let skip = first_quot_len + indent_length(&text[first_quot_len..]);
    squeeze(&text, skip, punct, brackets)
}

/// nano's `squeeze`: within `line[skip..]`, collapse runs of blanks to a
/// single space, except that up to two consecutive blanks survive right
/// after a punctuation (plus optional closing-bracket) character, and
/// strip trailing blanks entirely.
fn squeeze(line: &[char], skip: usize, punct: &str, brackets: &str) -> Vec<char> {
    let mut out: Vec<char> = line[..skip].to_vec();
    let body = &line[skip..];
    let mut i = 0;
    while i < body.len() {
        let c = body[i];
        if is_blank(c) {
            out.push(' ');
            i += 1;
            while i < body.len() && is_blank(body[i]) {
                i += 1;
            }
        } else if !punct.is_empty() && punct.contains(c) {
            out.push(c);
            i += 1;
            if i < body.len() && !brackets.is_empty() && brackets.contains(body[i]) {
                out.push(body[i]);
                i += 1;
            }
            let mut spaces = 0;
            while i < body.len() && is_blank(body[i]) && spaces < 2 {
                out.push(' ');
                i += 1;
                spaces += 1;
            }
            while i < body.len() && is_blank(body[i]) {
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    while out.len() > skip && *out.last().unwrap() == ' ' {
        out.pop();
    }
    out
}

/// nano's `break_line`: the char offset (within `text`) at which to break
/// so the kept segment is at most `goal` chars, landing just past any run
/// of blanks at the break point -- or `None` when `text` already fits
/// within `goal`, or there's no blank anywhere to break at.
fn break_line(text: &[char], goal: usize) -> Option<usize> {
    let mut start = 0;
    while start < text.len() && is_blank(text[start]) {
        start += 1;
    }
    if text.len() <= goal {
        return None;
    }
    let mut last_blank = None;
    let mut i = start;
    while i < text.len() && i <= goal {
        if is_blank(text[i]) {
            last_blank = Some(i);
        }
        i += 1;
    }
    let first_blank = match last_blank {
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
    let mut pos = first_blank + 1;
    while pos < text.len() && is_blank(text[pos]) {
        pos += 1;
    }
    Some(pos)
}

/// nano's `rewrap_paragraph`: re-split `text` (already concatenated and
/// squeezed) into lines that each fit within `wrap_at`, prefixing every
/// line after the first with `lead` (the first line keeps whatever lead
/// it already had baked in by `concat_and_squeeze`).
fn rewrap(text: Vec<char>, lead: &[char], wrap_at: usize, trim_blanks: bool) -> Vec<Vec<char>> {
    let mut lines = Vec::new();
    let mut cur = text;
    let lead_len = lead.len();
    loop {
        if cur.len() <= wrap_at {
            lines.push(cur);
            break;
        }
        let skip = lead_len.min(cur.len());
        let goal = wrap_at.saturating_sub(lead_len);
        let Some(rel_break) = break_line(&cur[skip..], goal) else {
            lines.push(cur);
            break;
        };
        let break_pos = skip + rel_break;
        if break_pos >= cur.len() {
            lines.push(cur);
            break;
        }
        let mut segment_end = break_pos;
        if trim_blanks {
            while segment_end > 0 && cur[segment_end - 1] == ' ' {
                segment_end -= 1;
            }
        }
        lines.push(cur[..segment_end].to_vec());
        let mut next_line = lead.to_vec();
        next_line.extend_from_slice(&cur[break_pos..]);
        cur = next_line;
    }
    lines
}

/// nano's `justify_paragraph`: justify the paragraph of `count` lines
/// starting at `lines[start]`, returning its replacement lines.
#[allow(clippy::too_many_arguments)]
pub fn justify_paragraph(
    lines: &[Vec<char>],
    start: usize,
    count: usize,
    quote_re: Option<&Regex>,
    punct: &str,
    brackets: &str,
    wrap_at: usize,
    trim_blanks: bool,
) -> Vec<Vec<char>> {
    let sample = if count == 1 {
        &lines[start]
    } else {
        &lines[start + 1]
    };
    let sample_quot_len = quote_length(sample, quote_re);
    let lead_len = sample_quot_len + indent_length(&sample[sample_quot_len..]);
    let lead: Vec<char> = sample[..lead_len].to_vec();

    let squeezed = concat_and_squeeze(lines, start, count, quote_re, punct, brackets);
    rewrap(squeezed, &lead, wrap_at, trim_blanks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<Vec<char>> {
        s.lines().map(|l| l.chars().collect()).collect()
    }

    fn to_strings(lines: &[Vec<char>]) -> Vec<String> {
        lines.iter().map(|l| l.iter().collect()).collect()
    }

    fn no_quote() -> Option<Regex> {
        None
    }

    #[test]
    fn bookstyle_makes_a_line_starting_with_whitespace_begin_a_paragraph() {
        // Checked against the installed nano's M-J with and without -O.
        let lines = chars("  aaa bbb\n  ccc ddd\n  eee\n> fff\n>  ggg\n>  hhh\n");
        let q = Regex::new(r"^([ \t]*([!#%:;>|}]|//))+").ok();
        let paras = |bookstyle| {
            let mut found = Vec::new();
            let mut at = 0;
            while let Some((start, count)) = find_paragraph(&lines, at, q.as_ref(), bookstyle) {
                found.push((start, count));
                at = start + count;
            }
            found
        };
        assert_eq!(paras(false), vec![(0, 3), (3, 3)]);
        // A quoted line's own indent comes after its `>`, so it doesn't count.
        assert_eq!(paras(true), vec![(0, 1), (1, 1), (2, 1), (3, 3)]);
        assert_eq!(para_begin(&lines, 2, q.as_ref(), true), 1);
        assert_eq!(para_end(&lines, 1, q.as_ref(), true), 1);
    }

    #[test]
    fn wrap_at_matches_nano_defaults() {
        assert_eq!(wrap_at(-8, 80), 72);
        assert_eq!(wrap_at(40, 80), 40);
        assert_eq!(wrap_at(-200, 80), 0);
    }

    #[test]
    fn find_paragraph_skips_blank_lines_and_stops_at_the_next_blank() {
        let lines = chars("\none two three\nfour five\n\nnext para\n");
        let found = find_paragraph(&lines, 0, no_quote().as_ref(), false);
        assert_eq!(found, Some((1, 2)));
    }

    #[test]
    fn find_paragraph_returns_none_past_the_last_paragraph() {
        let lines = chars("text\n\n");
        assert_eq!(find_paragraph(&lines, 2, no_quote().as_ref(), false), None);
    }

    #[test]
    fn justify_paragraph_wraps_to_the_fill_width() {
        // Confirmed against the installed nano's own output (`nano -r 20`,
        // Justify, then saved to disk): a break at a blank sitting exactly
        // on the goal column keeps that blank as a trailing space on the
        // line it breaks from, rather than trimming it (that's what
        // `set trimblanks` is for).
        let lines = chars("one two three four five six seven eight nine ten");
        let out = justify_paragraph(&lines, 0, 1, no_quote().as_ref(), "", "", 20, false);
        assert_eq!(
            to_strings(&out),
            vec![
                "one two three four ".to_string(),
                "five six seven eight ".to_string(),
                "nine ten".to_string(),
            ]
        );
    }

    #[test]
    fn justify_paragraph_joins_multiple_lines_into_one_before_rewrapping() {
        let lines = chars("one two\nthree four\nfive six");
        let out = justify_paragraph(&lines, 0, 3, no_quote().as_ref(), "", "", 80, false);
        assert_eq!(
            to_strings(&out),
            vec!["one two three four five six".to_string()]
        );
    }

    #[test]
    fn justify_paragraph_preserves_a_quote_prefix_on_every_line() {
        let quote_re = Regex::new("^(> )+").unwrap();
        let lines = chars("> one two three four five six seven eight nine ten");
        let out = justify_paragraph(
            &lines,
            0,
            1,
            Some(&quote_re),
            "",
            "",
            15, // small width forces a wrap
            false,
        );
        let strings = to_strings(&out);
        assert!(strings.len() > 1);
        assert!(strings.iter().all(|l| l.starts_with("> ")));
    }

    #[test]
    fn squeeze_collapses_runs_of_blanks_and_trims_trailing_ones() {
        let line: Vec<char> = "  a    b   ".chars().collect();
        assert_eq!(
            to_strings(&[squeeze(&line, 0, "", "")]),
            vec![" a b".to_string()]
        );
    }

    #[test]
    fn squeeze_keeps_up_to_two_spaces_after_punctuation() {
        let line: Vec<char> = "end.   next".chars().collect();
        assert_eq!(
            to_strings(&[squeeze(&line, 0, ".", "")]),
            vec!["end.  next".to_string()]
        );
    }
}
