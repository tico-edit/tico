//! Tree-sitter-based syntax highlighting: language detection (filename ->
//! shebang -> modeline, in that order, all on by default) and highlight-span
//! computation.
//!
//! Highlight queries are vendored locally under `src/syntax/queries/`
//! rather than pulled from each crate at run time: most `tree-sitter-*`
//! crates on crates.io do bundle their grammar's real `queries/highlights.scm`,
//! but not consistently — the constant that exposes it is named differently
//! from crate to crate (`HIGHLIGHTS_QUERY`, `HIGHLIGHT_QUERY`,
//! `XML_HIGHLIGHT_QUERY`, ...), sometimes commented out, and a few crates
//! (graphql, nim) don't publish one at all. Vendoring the same files
//! locally (see `queries/README.md`) sidesteps all of that and gives every
//! language a uniform `include_str!`.

mod languages;

pub use languages::{LanguageDef, detect, find_by_name, names};

/// Resolve a buffer's language, honoring an optional `-Y`/`--syntax` CLI
/// override (matching nano's `find_and_prime_applicable_syntax`): `"none"`
/// disables highlighting outright, a recognized name forces that language,
/// and an unrecognized name falls back to normal detection (nano shows an
/// "Unknown syntax name" alert in that last case; tico just falls back
/// silently, since there's no persistent status line to put it on this
/// early in startup).
pub fn detect_with_override(
    path: Option<&std::path::Path>,
    text: &str,
    syntax_override: Option<&str>,
) -> Option<&'static LanguageDef> {
    match syntax_override {
        Some("none") => None,
        Some(name) => find_by_name(name).or_else(|| detect(path, text)),
        None => detect(path, text),
    }
}

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tree_sitter::StreamingIterator;

/// An interned highlight scope name in Helix's vocabulary
/// (`keyword.control.import`, `constant.numeric`, `markup.heading`, ...).
/// The theme (`crate::theme`) maps scopes to styles by longest dotted
/// prefix, so the full capture name is kept — a theme may well style
/// `keyword.control.import` differently from plain `keyword`.
///
/// Interning keeps `HighlightSpan` small and `Copy` (a `u16` rather than a
/// heap string per span, of which a big buffer has tens of thousands) and
/// gives the theme a cheap key to memoize resolution on. The set of
/// distinct names is bounded by what the vendored queries contain, a few
/// hundred at most, so leaking them is fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scope(u16);

struct Interner {
    names: Vec<&'static str>,
    ids: HashMap<&'static str, u16>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(|| {
        Mutex::new(Interner {
            names: Vec::new(),
            ids: HashMap::new(),
        })
    })
}

impl Scope {
    /// The scope for a (Helix-vocabulary) name, interning it if new.
    pub fn intern(name: &str) -> Scope {
        let mut it = interner().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = it.ids.get(name) {
            return Scope(id);
        }
        let id = u16::try_from(it.names.len()).expect("more than 65535 distinct highlight scopes");
        let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
        it.names.push(leaked);
        it.ids.insert(leaked, id);
        Scope(id)
    }

    pub fn name(self) -> &'static str {
        interner().lock().unwrap_or_else(|e| e.into_inner()).names[self.0 as usize]
    }
}

/// One highlighted span of the buffer, as byte offsets into its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub scope: Scope,
    /// `LanguageDef::name` of the grammar whose query produced this span.
    /// Usually the buffer's own language, but a heredoc body injected with
    /// another language (`<<SQL` in Perl) carries that language, so the
    /// renderer can paint it with *its* theme rather than the buffer's.
    pub language: &'static str,
}

/// Parse `text` with `lang`'s grammar and run its highlight query, tagging
/// each capture with its (normalized) scope. Spans are returned in the
/// query's natural (mostly outer-to-inner) order, so painting them in that
/// order — later spans overwriting earlier ones where they overlap — gives
/// the expected "innermost/most-specific wins" result (e.g. an interpolated
/// variable inside a string shows as a variable, the rest of the string
/// still shows as a string). Returns an empty vec if parsing or compiling
/// the query fails (should not normally happen for a vendored, tested
/// query, but a corrupt/huge buffer shouldn't be able to crash the editor).
pub fn highlight(text: &str, lang: &'static LanguageDef) -> Vec<HighlightSpan> {
    let language = (lang.language)();
    let Some(tree) = parse(text, &language) else {
        return Vec::new();
    };
    let Ok(query) = tree_sitter::Query::new(&language, lang.highlights_query) else {
        return Vec::new();
    };

    // Capture index -> scope, resolved once per query rather than once per
    // captured node (there are as many of those as tokens in the file).
    let scopes: Vec<Option<Scope>> = query
        .capture_names()
        .iter()
        .map(|name| normalize_capture(name).map(|n| Scope::intern(&n)))
        .collect();

    let mut spans = Vec::new();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), text.as_bytes());
    while let Some((m, capture_ix)) = captures.next() {
        let capture = m.captures[*capture_ix];
        if let Some(scope) = scopes[capture.index as usize] {
            spans.push(HighlightSpan {
                start: capture.node.start_byte(),
                end: capture.node.end_byte(),
                scope,
                language: lang.name,
            });
        }
    }

    // Fallback pass: some grammars don't give numeric literals their own
    // named node, so a query can't capture them at all. Catch any leaf
    // token that looks like a number and isn't already covered by a real
    // capture.
    add_numeric_fallback(&tree, text, lang.name, &mut spans);

    // Heredoc language injection: when a heredoc's terminator names a known
    // language (e.g. `<<SQL`, `<<'HTML'`), or an enclosing Perl `use Inline
    // LANG => <<'END'` does, re-highlight its body with that language's own
    // grammar instead of leaving it as one flat string.
    inject_tree_heredocs(&tree, text, &mut spans);

    // Perl `use Inline C => q{ ... }`: the string is C.
    inject_inline_strings(&tree, text, &mut spans);

    // Perl `__DATA__` laid out as `@@ name` parts: highlight each part with
    // the language its name would get as a file.
    inject_data_sections(&tree, text, &mut spans);

    // Varnish inline C: highlight the body of a `C{ ... }C` block with the C
    // grammar instead of leaving it one flat string.
    inject_inline_c(&tree, text, &mut spans);

    spans
}

fn parse(text: &str, language: &tree_sitter::Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(text, None)
}

/// Replace whatever the outer grammar made of `text[start..end]` with the
/// spans `lang`'s own grammar produces for it, offset back into `text`.
/// If that comes to nothing, the outer coloring (typically one flat
/// string) is left in place rather than blanking the range out; returns
/// whether anything was injected.
fn inject_range(
    text: &str,
    spans: &mut Vec<HighlightSpan>,
    start: usize,
    end: usize,
    lang: &'static LanguageDef,
) -> bool {
    if end <= start || end > text.len() {
        return false;
    }
    let inner = highlight(&text[start..end], lang);
    if inner.is_empty() {
        return false;
    }
    spans.retain(|s| !(s.start < end && s.end > start));
    spans.extend(inner.into_iter().map(|s| HighlightSpan {
        start: s.start + start,
        end: s.end + start,
        ..s
    }));
    true
}

/// Heredoc language injection for Perl (`heredoc_token` or
/// `command_heredoc_token` / `heredoc_content`), Bash (`heredoc_start` /
/// `heredoc_body`), Ruby (`heredoc_beginning` / `heredoc_body`) and PHP
/// (`heredoc_start` / `heredoc_body`, or `nowdoc_body` for a nowdoc):
/// these grammars give every heredoc body its own node, wherever the
/// heredoc sits, so each start marker is paired with the first unclaimed
/// body after it -- bodies follow their markers in source order, including
/// several on one line. Perl's and Ruby's body node ends with the
/// `heredoc_end` terminator, which is trimmed off. The body's language is
/// the one its terminator names (`<<SQL`, `<<~'VCL'`), or in Perl the one
/// an enclosing `use Inline LANG => <<'END'` names. No other vendored
/// grammar produces these kinds, so this is a no-op elsewhere.
fn inject_tree_heredocs(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut starts = Vec::new();
    for kind in [
        "heredoc_token",
        "command_heredoc_token",
        "heredoc_start",
        "heredoc_beginning",
    ] {
        collect_by_kind(tree.root_node(), kind, &mut starts);
    }
    if starts.is_empty() {
        return;
    }
    let mut bodies = Vec::new();
    for kind in ["heredoc_content", "heredoc_body", "nowdoc_body"] {
        collect_by_kind(tree.root_node(), kind, &mut bodies);
    }
    starts.sort_by_key(|n| n.start_byte());
    bodies.sort_by_key(|n| n.start_byte());

    let mut next_body = 0;
    for start_id in starts {
        let Some(i) = bodies
            .iter()
            .skip(next_body)
            .position(|b| b.start_byte() >= start_id.end_byte())
        else {
            break;
        };
        let body = bodies[next_body + i];
        next_body += i + 1;

        let raw = &text[start_id.byte_range()];
        let Some(lang) = inline_language_before(start_id, text)
            .or_else(|| languages::find_by_name(heredoc_language_name(raw)))
        else {
            continue;
        };
        let mut body_end = body.end_byte();
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "heredoc_end" {
                body_end = child.start_byte();
                break;
            }
        }
        inject_range(text, spans, body.start_byte(), body_end, lang);
    }
}

/// The language a Perl `use Inline LANG => ...` statement names for the
/// source `node` that follows the `=>`, when there is one: `node` sits in
/// the `use_statement`'s `list_expression`, right after a `=>` whose left
/// side is `LANG`, an autoquoted bareword or a string. `LANG` is Inline's
/// name for the language (`C`, `CPP`, `Python`), matched
/// case-insensitively against tico's.
fn inline_language_before(node: tree_sitter::Node, text: &str) -> Option<&'static LanguageDef> {
    let list = node.parent()?;
    if list.kind() != "list_expression" {
        return None;
    }
    let arrow = node.prev_sibling()?;
    if arrow.kind() != "=>" {
        return None;
    }
    let lang = arrow.prev_sibling()?;
    let name = match lang.kind() {
        "autoquoted_bareword" => &text[lang.byte_range()],
        "string_literal" | "interpolated_string_literal" => {
            text[lang.byte_range()].trim_matches(['\'', '"'])
        }
        _ => return None,
    };
    let use_stmt = list.parent()?;
    if use_stmt.kind() != "use_statement" {
        return None;
    }
    let module = use_stmt.child_by_field_name("module")?;
    if &text[module.byte_range()] != "Inline" {
        return None;
    }
    languages::find_by_name(name)
}

/// Perl `use Inline C => q{ ... }` and the like: the string after the
/// `=>` is source in that language, so highlight it as such. A `q{}` or
/// `qq{}` string always is; a `'...'` or `"..."` string only when it
/// spans lines, since a one-line one names a file, or `DATA` (whose
/// code `inject_data_sections` finds under its `__C__` marker). A heredoc
/// there is handled by `inject_tree_heredocs`.
fn inject_inline_strings(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut strings = Vec::new();
    for kind in ["string_literal", "interpolated_string_literal"] {
        collect_by_kind(tree.root_node(), kind, &mut strings);
    }
    for node in strings {
        let Some(lang) = inline_language_before(node, text) else {
            continue;
        };
        // The text between the delimiters; an empty string has none.
        let Some(content) = node.child_by_field_name("content") else {
            continue;
        };
        let quote_like = node
            .child(0)
            .is_some_and(|c| matches!(c.kind(), "q" | "qq"));
        if !quote_like && !text[content.byte_range()].contains('\n') {
            continue;
        }
        inject_range(text, spans, content.start_byte(), content.end_byte(), lang);
    }
}

/// Perl `__DATA__`/`__END__` sections (`data_section`; no other
/// vendored grammar produces that kind, so this is a no-op elsewhere),
/// in either of the two layouts that put code there:
///
/// - Mojo::Loader, Data::Section::Simple and Data::Section::Pluggable:
///   each `@@ name` line starts a named part that runs to the next such
///   line. A part is highlighted with whatever language its name would
///   get as a file (`@@ hello.json` as JSON), or left plain when the name
///   says nothing, or when it carries a `(base64)` encoding as in
///   `@@ hello.bin (base64)`, since the text isn't the data. The `@@`,
///   the name and the encoding are colored as the markers they are.
/// - Inline (`use Inline C => 'DATA'`): each `__C__` line, the language's
///   Inline name in double underscores, starts a part in that language.
///
/// Text before the first marker is plain, and an `__END__` line ends the
/// data section as it does for all of those modules.
fn inject_data_sections(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut sections = Vec::new();
    collect_by_kind(tree.root_node(), "data_section", &mut sections);
    if sections.is_empty() {
        return;
    }
    let marker = Scope::intern("punctuation.special");
    let path = Scope::intern("string.special.path");
    let directive = Scope::intern("keyword.directive");
    for section in sections {
        let (start, end) = (section.start_byte(), section.end_byte().min(text.len()));
        if end <= start {
            continue;
        }
        let parts = data_section_parts(text, start, end);
        if !parts.is_empty() {
            // The query colors a data section as one comment; laid out in
            // parts, each part is colored on its own below (or left plain).
            spans.retain(|s| !(s.start == section.start_byte() && s.end == section.end_byte()));
        }
        for (i, part) in parts.iter().enumerate() {
            let body_end = parts.get(i + 1).map_or(part.end, |next| next.line_start);
            let name = &text[part.name_start..part.name_end];
            let lang = if part.inline {
                spans.push(HighlightSpan {
                    start: part.line_start,
                    end: part.name_end + 2,
                    scope: directive,
                    language: "perl",
                });
                languages::find_by_name(name)
            } else {
                spans.push(HighlightSpan {
                    start: part.line_start,
                    end: part.line_start + 2,
                    scope: marker,
                    language: "perl",
                });
                spans.push(HighlightSpan {
                    start: part.name_start,
                    end: part.name_end,
                    scope: path,
                    language: "perl",
                });
                if let Some((enc_start, enc_end)) = part.encoding {
                    spans.push(HighlightSpan {
                        start: enc_start,
                        end: enc_end,
                        scope: directive,
                        language: "perl",
                    });
                    continue;
                }
                let body = &text[part.body_start..body_end];
                languages::detect(Some(std::path::Path::new(name)), body)
            };
            if let Some(lang) = lang {
                inject_range(text, spans, part.body_start, body_end, lang);
            }
        }
    }
}

/// One part of a data section, as byte offsets into the buffer: an
/// `@@ name` part, or (`inline`) an Inline `__LANG__` part.
struct DataSectionPart {
    /// Whether this is an Inline `__LANG__` marker rather than `@@ name`.
    inline: bool,
    /// Where the marker line starts (its `@@` or `__`).
    line_start: usize,
    /// The name after `@@`, or the `LANG` between the underscores.
    name_start: usize,
    name_end: usize,
    /// The `(base64)` after an `@@` name, when present.
    encoding: Option<(usize, usize)>,
    /// The line after the marker; the body runs from here to the next
    /// part's `line_start`, or to `end` for the last part.
    body_start: usize,
    end: usize,
}

/// The parts of `text[start..end]`: `@@ name` lines, following
/// Data::Section::Pluggable's `^@@\s+(.+?)\s*\r?\n` split and its
/// `^(.*)\s+\((.*?)\)$` split of the name from an encoding, and Inline's
/// `__LANG__` marker lines. The parts stop at a line that is exactly
/// `__END__`.
fn data_section_parts(text: &str, start: usize, end: usize) -> Vec<DataSectionPart> {
    let mut parts = Vec::new();
    let mut end = end;
    let mut pos = start;
    while pos < end {
        let line_end = text[pos..end].find('\n').map_or(end, |i| pos + i);
        let next = (line_end + 1).min(end);
        let line = text[pos..line_end]
            .strip_suffix('\r')
            .unwrap_or(&text[pos..line_end]);
        if line == "__END__" {
            end = pos;
            break;
        }
        if let Some(name) = line.strip_prefix("__").and_then(|l| l.strip_suffix("__"))
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')
        {
            parts.push(DataSectionPart {
                inline: true,
                line_start: pos,
                name_start: pos + 2,
                name_end: pos + 2 + name.len(),
                encoding: None,
                body_start: next,
                end,
            });
        } else if let Some(rest) = line.strip_prefix("@@")
            && rest.starts_with(char::is_whitespace)
        {
            let name_all = rest.trim();
            if name_all.is_empty() {
                pos = next;
                continue;
            }
            let name_start = pos + 2 + (rest.len() - rest.trim_start().len());
            let name_all_end = name_start + name_all.len();
            let (name_end, encoding) = match name_all
                .strip_suffix(')')
                .and_then(|s| s.rfind('('))
                .filter(|&i| i > 0 && name_all[..i].ends_with(char::is_whitespace))
            {
                Some(i) => (
                    name_start + name_all[..i].trim_end().len(),
                    Some((name_start + i, name_all_end)),
                ),
                None => (name_all_end, None),
            };
            parts.push(DataSectionPart {
                inline: false,
                line_start: pos,
                name_start,
                name_end,
                encoding,
                body_start: next,
                end,
            });
        }
        pos = next;
    }
    for part in &mut parts {
        part.end = end;
        part.body_start = part.body_start.min(end);
    }
    parts
}

/// VCL-specific: the vendored VCL grammar lexes a Varnish `C{ ... }C` block
/// as a single `inline_c` token (its query colors it as a string as a
/// fallback). Re-highlight the body with the C grammar and color the two
/// delimiters, so the block reads as the C it is. No other grammar produces
/// `inline_c` nodes, so this is a no-op elsewhere.
fn inject_inline_c(tree: &tree_sitter::Tree, text: &str, spans: &mut Vec<HighlightSpan>) {
    let mut blocks = Vec::new();
    collect_by_kind(tree.root_node(), "inline_c", &mut blocks);
    if blocks.is_empty() {
        return;
    }
    let Some(c) = languages::find_by_name("c") else {
        return;
    };
    let delimiter = Scope::intern("punctuation.special");
    for block in blocks {
        let (start, end) = (block.start_byte(), block.end_byte());
        // `C{` and `}C` are two bytes each; the token can't be shorter.
        if end < start + 4 || end > text.len() {
            continue;
        }
        let (body_start, body_end) = (start + 2, end - 2);
        let inner_spans = highlight(&text[body_start..body_end], c);
        spans.retain(|s| !(s.start < end && s.end > start));
        for (a, b) in [(start, body_start), (body_end, end)] {
            spans.push(HighlightSpan {
                start: a,
                end: b,
                scope: delimiter,
                language: "vcl",
            });
        }
        spans.extend(inner_spans.into_iter().map(|s| HighlightSpan {
            start: s.start + body_start,
            end: s.end + body_start,
            ..s
        }));
    }
}

/// Strip a heredoc marker down to the bare language name, whichever
/// grammar produced it: an optional leading `<<` (Ruby's marker node is the
/// whole `<<~SQL`; Perl's, Bash's and PHP's start after the operator),
/// then `~` (indented, `<<~SQL`) or `-` (Ruby's `<<-SQL`), then `\`
/// (Perl's no-interpolation bareword, `<<\SQL`), then matching surrounding
/// `'` or `"` quotes.
fn heredoc_language_name(raw: &str) -> &str {
    let s = raw.strip_prefix("<<").unwrap_or(raw);
    let s = s.strip_prefix(['~', '-']).unwrap_or(s);
    let s = s.strip_prefix('\\').unwrap_or(s);
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn collect_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == kind {
        out.push(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_by_kind(child, kind, out);
    }
}

fn add_numeric_fallback(
    tree: &tree_sitter::Tree,
    text: &str,
    language: &'static str,
    spans: &mut Vec<HighlightSpan>,
) {
    let bytes = text.as_bytes();
    let mut cursor = tree.walk();
    let mut nodes = Vec::new();
    collect_leaves(&mut cursor, &mut nodes);

    // "Is this leaf already covered by a real capture?" used to be a
    // `spans.iter().any(...)` linear scan run for *every* leaf -- with
    // both leaf count and span count growing with file size, that's
    // O(leaves * spans), i.e. quadratic, and made opening or editing a
    // large file dramatically slower than it needed to be.
    //
    // Every span here comes from a node of this same parse tree (this
    // runs before heredoc injection adds any that don't), so any two
    // spans either nest or are disjoint -- they can never partially
    // overlap. That laminar property is what makes a sorted-start sweep
    // with a stack of "currently enclosing" spans a correct, linear-time
    // (after an O(n log n) sort) replacement: a leaf is already covered
    // exactly when the stack is non-empty once expired (ended-before-here)
    // entries have been popped off it.
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i].start);
    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::new();

    for node in nodes {
        let (start, end) = (node.start_byte(), node.end_byte());
        if end <= start || end > bytes.len() {
            continue;
        }
        while next < order.len() && spans[order[next]].start <= start {
            let idx = order[next];
            next += 1;
            // Drop anything on the stack that already ended before this
            // span begins -- a disjoint sibling, not really an enclosing
            // span, so it must not linger and cause false "covered" hits
            // for later leaves.
            while let Some(&top) = active.last()
                && spans[top].end <= spans[idx].start
            {
                active.pop();
            }
            active.push(idx);
        }
        while let Some(&top) = active.last()
            && spans[top].end <= start
        {
            active.pop();
        }
        if !active.is_empty() {
            continue;
        }
        if let Ok(leaf_text) = std::str::from_utf8(&bytes[start..end])
            && looks_numeric(leaf_text)
        {
            spans.push(HighlightSpan {
                start,
                end,
                scope: Scope::intern("constant.numeric"),
                language,
            });
        }
    }
}

fn collect_leaves<'a>(
    cursor: &mut tree_sitter::TreeCursor<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    loop {
        if cursor.node().child_count() == 0 {
            out.push(cursor.node());
        } else if cursor.goto_first_child() {
            collect_leaves(cursor, out);
            cursor.goto_parent();
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn looks_numeric(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let t = t
        .strip_prefix('-')
        .or_else(|| t.strip_prefix('+'))
        .unwrap_or(t);
    if t.is_empty() {
        return false;
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    let mut seen_digit = false;
    let mut seen_dot = false;
    for c in t.chars() {
        match c {
            '0'..='9' => seen_digit = true,
            '_' => {}
            '.' if !seen_dot => seen_dot = true,
            'e' | 'E' if seen_digit => {}
            _ => return false,
        }
    }
    seen_digit
}

/// Translate a raw tree-sitter capture name into Helix's scope vocabulary
/// (the one themes are written against), or `None` for captures that
/// shouldn't produce a highlight at all.
///
/// The vendored queries come from each grammar's own upstream repo, so they
/// are a mix of conventions: the tree-sitter CLI's (`function.method`,
/// `variable.builtin` — already Helix-compatible), nvim-treesitter's
/// (`conditional`, `repeat`, `include`, `number`, `property`, `text.title`
/// ...), and the odd grammar-specific one (elm suffixes everything with
/// `.elm`). Everything here is a *renaming*; the actual choice of color is
/// entirely the theme's, via longest-prefix lookup, so a name that has no
/// alias is passed through unchanged and still falls back sensibly
/// (`constant.macro` -> a theme's `constant`).
///
/// Captures dropped outright: `_`-prefixed helper captures (used only for
/// predicates), `spell`/`nospell`, `none`, `error`, `embedded`, and the
/// nvim `text.{note,warning,danger}` TODO markers, which have no Helix
/// equivalent — leaving them out means the enclosing comment's style shows
/// through, which is what a theme user would expect.
fn normalize_capture(raw: &str) -> Option<String> {
    let name = raw.strip_suffix(".elm").unwrap_or(raw);
    // `_foo`: predicate-only helper captures; `source.*`: injection markers.
    if name.starts_with('_') || name.starts_with("source.") {
        return None;
    }
    // Exact renames first: these change more than the leading segment.
    let exact = match name {
        "spell" | "nospell" | "none" | "error" | "embedded" | "clean" | "text.note"
        | "text.warning" | "text.danger" => return None,
        // tree-sitter-powershell captures a whole array literal and a whole
        // assignment's right-hand side under these; neither has a Helix
        // scope, and painting a region that size one color would hide the
        // real tokens inside it.
        "array" | "assignvalue" => return None,
        "number" => "constant.numeric",
        "number.float" | "float" => "constant.numeric.float",
        "boolean" => "constant.builtin.boolean",
        "character" | "char" | "character.special" => "constant.character",
        "escape" | "string.escape" | "character.escape" => "constant.character.escape",
        "string.regex" | "string.special.regex" => "string.regexp",
        "string.special.uri" => "string.special.url",
        "string.documentation" => "string",
        "conditional" | "keyword.conditional" | "keyword.conditional.ternary" => {
            "keyword.control.conditional"
        }
        "repeat" | "keyword.repeat" => "keyword.control.repeat",
        "include" | "import" | "keyword.import" | "meta.import" => "keyword.control.import",
        "exception" | "keyword.exception" => "keyword.control.exception",
        "keyword.return" => "keyword.control.return",
        "keyword.control" => "keyword.control",
        "keyword.coroutine" | "keyword.debug" | "keyword.other" | "keyword.other.port" => "keyword",
        "keyword.type" | "storage.type" => "keyword.storage.type",
        "keyword.modifier" | "storageclass" | "type.qualifier" => "keyword.storage.modifier",
        "preproc" | "define" | "macro" | "custom_directive" => "keyword.directive",
        "method" | "method.call" | "function.method.call" | "function.method.builtin" => {
            "function.method"
        }
        "function.call" | "local.function" => "function",
        "function.macro.builtin" => "function.macro",
        "parameter" | "parameter.builtin" => "variable.parameter",
        "property" | "property.definition" | "field" | "variable.member" => "variable.other.member",
        "module" | "module.builtin" => "namespace",
        "type.definition" | "interface" | "union" => "type",
        "tag.attribute" => "attribute",
        "symbol" => "string.special.symbol",
        "delimiter" => "punctuation.delimiter",
        "comment.doc" | "comment.doc.__attribute__" | "comment.documentation" => {
            "comment.block.documentation"
        }
        "text.title" => "markup.heading",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.label",
        "text.literal" => "markup.raw",
        "text.emphasis" => "markup.italic",
        "text.strong" => "markup.bold",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact.to_string());
    }
    // Otherwise rename just the leading segment where nvim's differs from
    // Helix's, keeping any more specific tail (`property.foo` ->
    // `variable.other.member.foo`).
    let (head, tail) = match name.split_once('.') {
        Some((h, t)) => (h, Some(t)),
        None => (name, None),
    };
    let head = match head {
        "number" => "constant.numeric",
        "character" => "constant.character",
        "conditional" => "keyword.control.conditional",
        "repeat" => "keyword.control.repeat",
        "include" => "keyword.control.import",
        "exception" => "keyword.control.exception",
        "preproc" => "keyword.directive",
        "method" => "function.method",
        "parameter" => "variable.parameter",
        "property" | "field" => "variable.other.member",
        "module" => "namespace",
        "text" => "markup",
        other => other,
    };
    Some(match tail {
        Some(t) => format!("{head}.{t}"),
        None => head.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use languages::detect;

    /// Confirm every registered language's query actually compiles against
    /// its grammar (Query::new can fail at runtime even when the Rust code
    /// compiles fine, e.g. from a typo'd node name), and produces at least
    /// one non-empty span on a real snippet where that's a reasonable
    /// expectation.
    fn check(name: &str, path: &str, source: &str, want_at_least_one: bool) {
        let lang = languages::detect(Some(std::path::Path::new(path)), source)
            .unwrap_or_else(|| panic!("{name}: detection failed for {path}"));
        assert_eq!(lang.name, name, "detected wrong language for {path}");
        let spans = highlight(source, lang);
        if want_at_least_one {
            assert!(
                !spans.is_empty(),
                "{name}: expected at least one highlight span, got none"
            );
        }
    }

    #[test]
    fn perl_highlights() {
        check(
            "perl",
            "test.pl",
            "#!/usr/bin/env perl\nuse strict;\nmy $x = 42; # comment\nsub foo { return \"hi\"; }\n",
            true,
        );
    }

    /// The `$)` in a `sub f ($)` prototype is not the special variable
    /// `$)`. (The grammar tico used before read it as one, and its error
    /// recovery then left every string after it unterminated.)
    #[test]
    fn perl_prototype_is_not_a_special_variable() {
        let src = concat!(
            "sub f ($)\n",
            "{\n",
            "  my $pm = \"$class.pm\";\n",
            "  $pm =~ s/::/\\//g;\n",
            "  require $pm;\n",
            "}\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), src).unwrap();
        let spans = highlight(src, lang);
        let exact = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(exact("f"), Some(("function".to_string(), "perl")));
        assert_eq!(exact("\"$class.pm\""), Some(("string".to_string(), "perl")));
        assert_eq!(
            exact("require"),
            Some(("keyword.control.import".to_string(), "perl"))
        );
    }

    /// The vendored VCL grammar is a fork of ntsk/tree-sitter-vcl extended
    /// for Fastly's dialect (see `grammars/tree-sitter-vcl/README.md`).
    /// Every Fastly-only construct here must parse without an ERROR node,
    /// and the query must color the ones with dedicated captures.
    /// The Turbo Pascal constructs tico's fork of tree-sitter-pascal adds
    /// (see `grammars/tree-sitter-pascal/README.md`) parse without a
    /// single ERROR node, and the query paints them sensibly.
    #[test]
    fn pascal_turbo_pascal_dialect_parses_and_highlights() {
        let source = concat!(
            "{$M 16384,0,655360}\n",
            "{$I-}\n",
            "(* block comment *)\n",
            "program Demo(input, output);\n",
            "uses Crt;\n",
            "const\n",
            "  Hex = $FF;\n",
            "  Bell = ^G;\n",
            "  Big = 1.5E10;\n",
            "  Bin = %1010;\n",
            "  Oct = &777;\n",
            "  Msg = 'It''s'^M^J;\n",
            "type\n",
            "  TRange = 1..10;\n",
            "  TDigits = set of '0'..'9';\n",
            "  TSigned = -MaxInt..MaxInt;\n",
            "  TIdx = Low(TRange)..High(TRange);\n",
            "  PInt = ^Integer;\n",
            "var\n",
            "  Screen: array[0..3999] of Byte absolute $B800:$0000;\n",
            "  Shadow: Word absolute Hex;\n",
            "  P: PInt;\n",
            "  I: Integer;\n",
            "label 10, Done;\n",
            "procedure Handler; interrupt;\n",
            "begin\n",
            "  Port[$20] := $20;\n",
            "end;\n",
            "begin\n",
            "  { brace comment }\n",
            "  P^ := P^ + 1;\n",
            "  for I := 1 to 10 do\n",
            "    if I mod 2 = 0 then goto 10;\n",
            "  case I of\n",
            "    1..9: WriteLn('small');\n",
            "  otherwise\n",
            "    Exit;\n",
            "  end;\n",
            "  goto Done;\n",
            "10:\n",
            "  Write(^G, #7);\n",
            "Done:\n",
            "end.\n",
        );
        let lang = languages::find_by_name("pascal").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "Turbo Pascal failed to parse:\n{}",
            tree.root_node().to_sexp()
        );

        let spans = highlight(source, lang);
        let scope_of = |needle: &str| {
            let start = source.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(
            scope_of("{$M 16384,0,655360}").as_deref(),
            Some("keyword.directive")
        );
        assert_eq!(scope_of("(* block comment *)").as_deref(), Some("comment"));
        assert_eq!(scope_of("{ brace comment }").as_deref(), Some("comment"));
        assert_eq!(scope_of("program").as_deref(), Some("keyword"));
        assert_eq!(scope_of("$FF").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("1.5E10").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("%1010").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("&777").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("^G").as_deref(), Some("string"));
        assert_eq!(scope_of("'It''s'^M^J").as_deref(), Some("string"));
        assert_eq!(scope_of("TRange").as_deref(), Some("type"));
        assert_eq!(scope_of("MaxInt").as_deref(), Some("constant"));
        assert_eq!(scope_of("Byte").as_deref(), Some("type"));
        assert_eq!(scope_of("absolute").as_deref(), Some("keyword"));
        assert_eq!(scope_of("$B800").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("Handler").as_deref(), Some("function"));
        assert_eq!(scope_of("interrupt").as_deref(), Some("attribute"));
        assert_eq!(scope_of("for").as_deref(), Some("keyword.control.repeat"));
        assert_eq!(
            scope_of("if").as_deref(),
            Some("keyword.control.conditional")
        );
        assert_eq!(scope_of("mod").as_deref(), Some("keyword.operator"));
        assert_eq!(scope_of("WriteLn").as_deref(), Some("function"));
        assert_eq!(
            scope_of("otherwise").as_deref(),
            Some("keyword.control.conditional")
        );
        assert_eq!(scope_of("Exit").as_deref(), Some("keyword.control.return"));
        assert_eq!(scope_of("10:").as_deref(), Some("constant"));
        assert_eq!(scope_of("Done:").as_deref(), Some("constant"));
    }

    #[test]
    fn vcl_fastly_dialect_parses_and_highlights() {
        let source = concat!(
            "pragma optional_param geoip_opt_in true;\n",
            "backend F_origin {\n",
            "  .host = \"origin.example.com\";\n",
            "  .ssl = true;\n",
            "  .probe = { .request = \"HEAD / HTTP/1.1\" \"Connection: close\"; .timeout = 2s; }\n",
            "}\n",
            "table redirects { \"/old\": \"/new\", }\n",
            "table limits INTEGER { \"max\": 100 }\n",
            "director pool random { .quorum = 20%; { .backend = F_origin; .weight = 1; } }\n",
            "penaltybox pb {}\n",
            "ratecounter rc {}\n",
            "sub is_admin BOOL { return req.http.Cookie:admin == \"1\"; }\n",
            "sub vcl_recv {\n",
            "#FASTLY recv\n",
            "  declare local var.x STRING;\n",
            "  set var.x = std.tolower(req.http.Host) \"/\" req.url;\n",
            "  set req.hash += req.url;\n",
            "  set req.http.X = if(req.url ~ \"^/x\", \"yes\", \"no\");\n",
            "  if ((req.url ~ \"^/y\") && !req.http.Z) { error 601 var.x; }\n",
            "  add req.http.Vary = \"Accept\";\n",
            "  remove req.http.Cookie;\n",
            "  goto done;\n",
            "  done:\n",
            "  return (lookup);\n",
            "}\n",
            "sub vcl_fetch {\n",
            "  if (beresp.status == 503 && req.restarts < 1) { restart; }\n",
            "  esi;\n",
            "  return (deliver);\n",
            "}\n",
            "sub vcl_error {\n",
            "  synthetic {\"<p>\"} obj.status {\"</p>\"};\n",
            "  log \"syslog \" req.service_id \" x :: \" req.url;\n",
            "  include \"snippet\";\n",
            "  return (deliver);\n",
            "}\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "Fastly VCL failed to parse:\n{}",
            tree.root_node().to_sexp()
        );

        // The renderer paints spans in order, so for a node captured more
        // than once the last span wins; mirror that here.
        let spans = highlight(source, lang);
        let scope_of = |needle: &str| {
            let start = source.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(
            scope_of("#FASTLY recv").as_deref(),
            Some("keyword.directive")
        );
        assert_eq!(scope_of("table").as_deref(), Some("keyword"));
        assert_eq!(scope_of("declare").as_deref(), Some("keyword"));
        assert_eq!(scope_of("STRING").as_deref(), Some("type"));
        assert_eq!(scope_of("INTEGER").as_deref(), Some("type"));
        assert_eq!(scope_of("+=").as_deref(), Some("operator"));
        assert_eq!(scope_of("20%").as_deref(), Some("constant.numeric"));
        assert_eq!(scope_of("tolower").as_deref(), Some("function"));
        assert_eq!(scope_of("std").as_deref(), Some("namespace"));
        assert_eq!(scope_of("vcl_recv").as_deref(), Some("function.builtin"));
        assert_eq!(scope_of("lookup").as_deref(), Some("constant"));
        assert_eq!(scope_of("done").as_deref(), Some("label"));
        assert_eq!(
            scope_of(".quorum").as_deref(),
            Some("variable.other.member")
        );
        assert_eq!(scope_of("random").as_deref(), Some("type"));
    }

    /// Varnish `C{ ... }C` blocks: the body is re-highlighted as C (spans
    /// carrying the C language, so the renderer uses C's theme) and the
    /// delimiters get their own color, at top level and inside a sub.
    #[test]
    fn vcl_inline_c_is_highlighted_as_c() {
        let source = concat!(
            "C{\n",
            "#include <stdio.h>\n",
            "static int counter = 0;\n",
            "}C\n",
            "sub vcl_recv {\n",
            "  C{ if (1) { counter++; } }C\n",
            "  return (pass);\n",
            "}\n",
            "C{}C\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );

        let spans = highlight(source, lang);
        let at = |needle: &str| {
            let start = source.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        // C keywords inside the block come from the C grammar and query.
        assert_eq!(at("static"), Some(("keyword".to_string(), "c")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        assert_eq!(at("if"), Some(("keyword".to_string(), "c")));
        // The delimiters are colored, and as VCL.
        assert_eq!(at("C{"), Some(("punctuation.special".to_string(), "vcl")));
        assert_eq!(at("}C"), Some(("punctuation.special".to_string(), "vcl")));
        // Nothing from the VCL query survives inside a block.
        let first_block_end = source.find("}C").unwrap() + 2;
        assert!(
            spans
                .iter()
                .filter(|s| s.start < first_block_end)
                .all(|s| s.language == "c" || s.scope.name() == "punctuation.special"),
            "VCL spans leaked into the C block"
        );
        // Statements after the block are still VCL.
        assert_eq!(
            at("return"),
            Some(("keyword.control.return".to_string(), "vcl"))
        );
    }

    /// An ordinary rule with no standard target name must still get color:
    /// the crate's make query left `foo: bar` blank (only the `:` was
    /// captured), which read as "highlighting is off" in a two-line Makefile.
    /// The Tcl query stacks captures (`@spell @comment`, `@repeat
    /// @keyword`); the Helix-scope one must be what wins for each node.
    #[test]
    fn tcl_highlights() {
        let src =
            "# note\nproc greet {name} {\n    puts \"hi $name\"\n}\nforeach x $list { incr n }\n";
        let lang = languages::detect(Some(std::path::Path::new("a.tcl")), src).unwrap();
        assert_eq!(lang.name, "tcl");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("# note").as_deref(), Some("comment"));
        assert_eq!(at("proc").as_deref(), Some("keyword.function"));
        assert_eq!(at("greet").as_deref(), Some("variable"));
        assert_eq!(at("puts").as_deref(), Some("function.builtin"));
        assert_eq!(at("\"hi $name\"").as_deref(), Some("string"));
        assert_eq!(at("foreach").as_deref(), Some("keyword.control.repeat"));
        assert_eq!(at("incr").as_deref(), Some("function.builtin"));
    }

    #[test]
    fn dockerfile_highlights() {
        let src = "# note\nFROM alpine:3.20 AS base\nARG VERSION=1\nENV APP_HOME=/app\nRUN echo \"$VERSION\"\nEXPOSE 80\n";
        let lang = languages::detect(Some(std::path::Path::new("Dockerfile")), src).unwrap();
        assert_eq!(lang.name, "dockerfile");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        // The grammar's comment node includes its newline.
        assert_eq!(at("# note\n").as_deref(), Some("comment"));
        assert_eq!(at("FROM").as_deref(), Some("keyword"));
        assert_eq!(at("AS").as_deref(), Some("keyword"));
        assert_eq!(at("ARG").as_deref(), Some("keyword"));
        assert_eq!(at("VERSION").as_deref(), Some("variable.other.member"));
        assert_eq!(at("APP_HOME").as_deref(), Some("variable.other.member"));
        assert_eq!(at("80").as_deref(), Some("constant.numeric"));
        // A RUN body is one opaque shell_command node in this grammar (the
        // upstream query injects bash there, which tico doesn't do), so
        // nothing inside it is colored.
        assert!(
            spans
                .iter()
                .all(|s| s.start < src.find("echo").unwrap()
                    || s.start >= src.find("EXPOSE").unwrap())
        );
    }

    /// Template Toolkit highlights only its own `[% ... %]` directives;
    /// whatever the template generates (VCL here) is left plain.
    #[test]
    fn tt2_highlights_directives_only() {
        let src = "[%# header %]\nsub vcl_recv {\n[% FOREACH b IN backends.sort('name') -%]\n  set req.http.B = \"[% b.name | html %]\";\n[% END %]\n}\n";
        let lang = languages::detect(Some(std::path::Path::new("default.vcl.tt")), src).unwrap();
        assert_eq!(lang.name, "tt2");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("[%# header %]").as_deref(), Some("comment"));
        assert_eq!(at("FOREACH").as_deref(), Some("keyword"));
        assert_eq!(at("IN").as_deref(), Some("keyword"));
        assert_eq!(at("backends").as_deref(), Some("variable"));
        assert_eq!(at("sort").as_deref(), Some("function.method"));
        assert_eq!(at("'name'").as_deref(), Some("string"));
        assert_eq!(at("END").as_deref(), Some("keyword"));
        // Nothing in the VCL content is colored, not even things that
        // would be keywords or strings in VCL.
        let plain = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .all(|s| s.end <= start || s.start >= start + needle.len())
        };
        assert!(plain("sub vcl_recv {"));
        assert!(plain("set req.http.B = \""));
    }

    #[test]
    fn powershell_highlights() {
        let src = "# note\nfunction Get-Thing {\n    param([string]$Name)\n    $x = @(1, 2)\n    if ($Name -eq 'a') { Write-Host \"hi $Name\" }\n    return $x.Count\n}\n";
        let lang = languages::detect(Some(std::path::Path::new("a.ps1")), src).unwrap();
        assert_eq!(lang.name, "powershell");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("# note").as_deref(), Some("comment"));
        assert_eq!(at("function").as_deref(), Some("keyword"));
        assert_eq!(at("Get-Thing").as_deref(), Some("function"));
        assert_eq!(at("string").as_deref(), Some("type"));
        assert_eq!(at("$Name").as_deref(), Some("variable"));
        assert_eq!(at("-eq").as_deref(), Some("operator"));
        assert_eq!(at("'a'").as_deref(), Some("string"));
        assert_eq!(at("Write-Host").as_deref(), Some("function"));
        assert_eq!(at("Count").as_deref(), Some("variable.other.member"));
        // The query's `@array` / `@assignvalue` captures are dropped rather
        // than painted: the array literal and the assignment's right-hand
        // side get no span of their own, only their inner tokens do.
        assert!(!spans.iter().any(|s| &src[s.start..s.end] == "@(1, 2)"));
        assert_eq!(at("1").as_deref(), Some("constant.numeric"));
    }

    #[test]
    fn batch_highlights() {
        let src = "@echo off\nREM note\n:: also\nset NAME=world\nif \"%NAME%\"==\"world\" (\n  echo Hello %NAME% %ERRORLEVEL%\n)\nfor %%f in (*.txt) do call :sub %%f\n:sub\necho %1 > out.txt\n";
        let lang = languages::detect(Some(std::path::Path::new("a.bat")), src).unwrap();
        assert_eq!(lang.name, "batch");
        for path in ["a.cmd", "a.BAT", "a.btm"] {
            assert_eq!(
                languages::detect(Some(std::path::Path::new(path)), "")
                    .unwrap()
                    .name,
                "batch"
            );
        }
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        // Like `at`, but locates the token by a longer unique context and
        // takes the span covering its first `len` bytes.
        let at_start_of = |context: &str, len: usize| {
            let start = src.find(context).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + len)
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("@echo off").as_deref(), Some("keyword"));
        assert_eq!(at("REM note").as_deref(), Some("comment"));
        assert_eq!(at(":: also").as_deref(), Some("comment"));
        assert_eq!(at("set").as_deref(), Some("keyword"));
        assert_eq!(at("NAME").as_deref(), Some("variable"));
        assert_eq!(at("==").as_deref(), Some("operator"));
        assert_eq!(at_start_of("echo Hello", 4).as_deref(), Some("function"));
        assert_eq!(
            at_start_of("%NAME% %ERRORLEVEL%", 6).as_deref(),
            Some("variable")
        );
        // The reordered builtin pattern wins over the generic one.
        assert_eq!(at("%ERRORLEVEL%").as_deref(), Some("variable.builtin"));
        assert_eq!(at("%%f").as_deref(), Some("variable.parameter"));
        assert_eq!(at_start_of(":sub\necho", 4).as_deref(), Some("label"));
        assert_eq!(at("out.txt").as_deref(), Some("string.special"));
    }

    /// The vendored CUE query has its generic `(identifier) @variable`
    /// pattern moved first so the specific field/type/function captures
    /// survive tico's last-wins painting.
    #[test]
    fn cue_highlights() {
        let src = "package app\n// A schema\n#Server: {\n\tname: string\n\tport: int & >0 | *8080\n\tmemory: 1.5Gi\n}\nservers: [for n in [\"web\"] { #Server & {name: strings.ToUpper(n)} }]\nif len(servers) > 1 { ha: true }\nnothing: null\n";
        let lang = languages::detect(Some(std::path::Path::new("a.cue")), src).unwrap();
        assert_eq!(lang.name, "cue");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("package").as_deref(), Some("keyword.control.import"));
        assert_eq!(at("app").as_deref(), Some("namespace"));
        assert_eq!(at("// A schema").as_deref(), Some("comment"));
        assert_eq!(at("#Server").as_deref(), Some("type"));
        assert_eq!(at("name").as_deref(), Some("variable.other.member"));
        assert_eq!(at("string").as_deref(), Some("type.builtin"));
        assert_eq!(at(">").as_deref(), Some("operator"));
        assert_eq!(at("8080").as_deref(), Some("constant.numeric"));
        assert_eq!(at("1.5").as_deref(), Some("constant.numeric.float"));
        assert_eq!(at("for").as_deref(), Some("keyword.control.repeat"));
        {
            // `in` also occurs inside `string`; find the keyword by context.
            let start = src.find(" in [").unwrap() + 1;
            let scope = spans
                .iter()
                .filter(|s| s.start == start && s.end == start + 2)
                .map(|s| s.scope.name().to_string())
                .next_back();
            assert_eq!(scope.as_deref(), Some("keyword.operator"));
        }
        assert_eq!(at("ToUpper").as_deref(), Some("function"));
        assert_eq!(at("len").as_deref(), Some("function.builtin"));
        assert_eq!(at("if").as_deref(), Some("keyword.control.conditional"));
        assert_eq!(at("true").as_deref(), Some("constant.builtin.boolean"));
        assert_eq!(at("null").as_deref(), Some("constant.builtin"));
    }

    #[test]
    fn hcl_terraform_highlights() {
        let src = "# note\nvariable \"name\" {\n  type    = string\n  default = null\n}\nresource \"aws_instance\" \"web\" {\n  ami   = var.ami\n  count = 2\n  tags  = { Name = \"web-${count.index}\" }\n  ids   = [for s in local.subnets : s.id if s.public]\n  size  = max(1, 2)\n  user_data = <<-EOT\n    hello\n  EOT\n}\n";
        let lang = languages::detect(Some(std::path::Path::new("main.tf")), src).unwrap();
        assert_eq!(lang.name, "hcl");
        for path in ["a.hcl", "terraform.tfvars", "job.nomad"] {
            assert_eq!(
                languages::detect(Some(std::path::Path::new(path)), "")
                    .unwrap()
                    .name,
                "hcl"
            );
        }
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        let at_start_of = |context: &str, len: usize| {
            let start = src.find(context).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + len)
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("# note").as_deref(), Some("comment"));
        assert_eq!(at("variable").as_deref(), Some("type.builtin"));
        assert_eq!(at("resource").as_deref(), Some("type.builtin"));
        assert_eq!(at("aws_instance").as_deref(), Some("string"));
        assert_eq!(at("string").as_deref(), Some("type.builtin"));
        assert_eq!(at("null").as_deref(), Some("constant.builtin"));
        assert_eq!(at("ami").as_deref(), Some("variable.other.member"));
        assert_eq!(
            at_start_of("var.ami", 3).as_deref(),
            Some("variable.builtin")
        );
        assert_eq!(at("2").as_deref(), Some("constant.numeric"));
        assert_eq!(at("${").as_deref(), Some("punctuation.special"));
        assert_eq!(at("index").as_deref(), Some("variable.other.member"));
        assert_eq!(at("for").as_deref(), Some("keyword.control.repeat"));
        assert_eq!(at("local").as_deref(), Some("variable.builtin"));
        assert_eq!(at("if").as_deref(), Some("keyword.control.conditional"));
        assert_eq!(at("max").as_deref(), Some("function.method"));
        assert_eq!(at("<<-").as_deref(), Some("punctuation.delimiter"));
        assert_eq!(at("hello").as_deref(), Some("string"));
    }

    /// The vendored V query is reordered for tico's last-wins painting, so
    /// a method call's name stays a method rather than a plain field.
    #[test]
    fn v_highlights() {
        let src = "module main\n\nimport os\n\n// A point\n@[heap]\npub struct Point {\nmut:\n\tx int\n}\n\nfn (p Point) dist(scale f64) f64 {\n\treturn p.x * scale + 1.5\n}\n\nfn main() {\n\tp := Point{x: 3}\n\tname := os.args[0]\n\tprintln('${name}: ${p.dist(2.0)}\\n')\n\tif true { exit(0) }\n}\n";
        let lang = languages::detect(Some(std::path::Path::new("a.v")), src).unwrap();
        assert_eq!(lang.name, "v");
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("module").as_deref(), Some("keyword.storage.type"));
        assert_eq!(at("main").as_deref(), Some("namespace"));
        assert_eq!(at("import").as_deref(), Some("keyword.control.import"));
        assert_eq!(at("os").as_deref(), Some("namespace"));
        assert_eq!(at("// A point").as_deref(), Some("comment"));
        assert_eq!(at("@[heap]").as_deref(), Some("attribute"));
        {
            let start = src.find("heap]").unwrap() + 4;
            let scope = spans
                .iter()
                .filter(|s| s.start == start && s.end == start + 1)
                .map(|s| s.scope.name().to_string())
                .next_back();
            assert_eq!(scope.as_deref(), Some("attribute"));
        }
        assert_eq!(at("pub").as_deref(), Some("keyword"));
        assert_eq!(at("Point").as_deref(), Some("type"));
        assert_eq!(at("mut").as_deref(), Some("keyword.storage.modifier.mut"));
        assert_eq!(at("x").as_deref(), Some("variable.other.member"));
        assert_eq!(at("f64").as_deref(), Some("type"));
        assert_eq!(at("dist").as_deref(), Some("function.method"));
        assert_eq!(at("scale").as_deref(), Some("variable.parameter"));
        assert_eq!(at("return").as_deref(), Some("keyword.control.return"));
        assert_eq!(at("1.5").as_deref(), Some("constant.numeric.float"));
        assert_eq!(at("3").as_deref(), Some("constant.numeric.integer"));
        assert_eq!(at("println").as_deref(), Some("function"));
        assert_eq!(at("${").as_deref(), Some("punctuation.bracket"));
        {
            // `p.dist(2.0)` inside the interpolation: a method call.
            let start = src.find("p.dist(").unwrap() + 2;
            let scope = spans
                .iter()
                .filter(|s| s.start == start && s.end == start + 4)
                .map(|s| s.scope.name().to_string())
                .next_back();
            assert_eq!(scope.as_deref(), Some("function.method"));
        }
        assert_eq!(at("\\n").as_deref(), Some("constant.character.escape"));
        assert_eq!(at("if").as_deref(), Some("keyword.control.conditional"));
        assert_eq!(at("true").as_deref(), Some("constant.builtin.boolean"));
    }

    #[test]
    fn v_detection() {
        let detect = |path: &str, text: &str| {
            languages::detect(Some(std::path::Path::new(path)), text).map(|l| l.name)
        };
        assert_eq!(detect("build.vsh", ""), Some("v"));
        assert_eq!(detect("v.mod", "Module {\n}\n"), Some("v"));
        assert_eq!(
            detect("script", "#!/usr/bin/env v\nprintln(1)\n"),
            Some("v")
        );
        assert_eq!(
            detect("script", "#!/usr/bin/env -S v run\nprintln(1)\n"),
            Some("v")
        );
    }

    #[test]
    fn make_plain_rule_is_colored() {
        let src = "foo: bar baz.o\n\tcc -o foo bar\n\nall: foo\n";
        let lang = languages::detect(Some(std::path::Path::new("Makefile")), src).unwrap();
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| s.scope.name().to_string())
                .next_back()
        };
        assert_eq!(at("foo").as_deref(), Some("function"));
        assert_eq!(at("bar").as_deref(), Some("string.special.path"));
        assert_eq!(at("baz.o").as_deref(), Some("string.special.path"));
        // A standard target keeps the crate's more specific capture.
        assert_eq!(at("all").as_deref(), Some("constant.macro"));
    }

    /// A Fastly VCL snippet is a bare run of statements with no `sub` around
    /// it, and that is also what a `<<VCL` heredoc usually holds. It must
    /// parse cleanly: when it went through error recovery instead, the
    /// lexer split hyphenated header names and left odd characters
    /// uncolored (`AWS-Access-Key-Id` lost its second `A`).
    #[test]
    fn vcl_snippet_without_sub_parses_and_keeps_hyphenated_names_whole() {
        let snippet = concat!(
            "set req.http.AWS-Access-Key-Id = \"AKIA\";\n",
            "set req.http.Slack-Bot-Token = \"xoxb\";\n",
            "if (req.url ~ \"^/x\") { error 404; }\n",
            "C{ int x; }C\n",
            "include \"snip\";\n",
        );
        let lang = languages::find_by_name("vcl").unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&(lang.language)()).unwrap();
        let tree = parser.parse(snippet, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "{}",
            tree.root_node().to_sexp()
        );

        // The same snippet as an indented, quoted heredoc inside a Perl hash.
        let src = format!(
            "my %h = (\n    content => <<~'VCL',\n{}    VCL\n);\n",
            snippet
                .lines()
                .map(|l| format!("        {l}\n"))
                .collect::<String>()
        );
        let perl = languages::detect(Some(std::path::Path::new("a.pl")), &src).unwrap();
        let spans = highlight(&src, perl);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(at("set"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(
            at("AWS-Access-Key-Id"),
            Some(("variable".to_string(), "vcl"))
        );
        assert_eq!(at("Slack-Bot-Token"), Some(("variable".to_string(), "vcl")));
        assert_eq!(at("error"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        // Every non-blank character of the header name lines is covered by
        // some VCL span, so nothing renders in the terminal's plain color.
        for line in [
            "set req.http.AWS-Access-Key-Id = \"AKIA\";",
            "set req.http.Slack-Bot-Token = \"xoxb\";",
        ] {
            let start = src.find(line).unwrap();
            for (i, ch) in line.char_indices() {
                if ch == ' ' {
                    continue;
                }
                let pos = start + i;
                assert!(
                    spans
                        .iter()
                        .any(|s| s.language == "vcl" && s.start <= pos && pos < s.end),
                    "{ch:?} at column {i} of {line:?} has no VCL span"
                );
            }
        }
    }

    #[test]
    fn numeric_fallback_finds_disjoint_and_nested_numbers() {
        // Perl doesn't give numeric literals their own captured node, so
        // these all rely on `add_numeric_fallback`'s sweep. Several plain
        // disjoint numbers plus one inside a nested capture (an array
        // index expression) exercises the exact shape of bug its
        // stack-based rewrite had to avoid: a sibling span that already
        // ended must not linger on the stack and cause a later, unrelated
        // leaf to be wrongly treated as "already covered".
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "my @a = (1, 22, 333); my $x = $a[444];\n";
        let spans = highlight(src, lang);
        for needle in ["1", "22", "333", "444"] {
            let start = src.find(needle).unwrap();
            let end = start + needle.len();
            assert!(
                spans.iter().any(|s| s.scope.name() == "constant.numeric"
                    && s.start == start
                    && s.end == end),
                "expected a constant.numeric span for {needle:?} at {start}..{end}, got: {spans:?}"
            );
        }
    }

    #[test]
    fn heredoc_injects_named_language() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "print <<SQL;\n   SELECT * FROM foo WHERE bar = 1\nSQL\n";
        let spans = highlight(src, lang);
        // "SELECT" and "FROM" should be captured as SQL keywords, at their
        // exact position within the outer Perl buffer -- not just colored
        // as one flat Perl string covering the whole heredoc body.
        let select_start = src.find("SELECT").unwrap();
        let from_start = src.find("FROM").unwrap();
        assert!(
            spans.iter().any(|s| s.scope.name().starts_with("keyword")
                && s.start == select_start
                && s.end == select_start + 6),
            "expected a keyword span for SELECT at {select_start}, got {spans:?}"
        );
        assert!(
            spans.iter().any(|s| s.scope.name().starts_with("keyword")
                && s.start == from_start
                && s.end == from_start + 4),
            "expected a keyword span for FROM at {from_start}, got {spans:?}"
        );
    }

    /// `<<VCL` in Perl injects the VCL grammar, and a `C{ ... }C` block inside
    /// that heredoc is injected as C in turn: highlight() recurses, so each
    /// layer's injections run on the body it was handed.
    #[test]
    fn heredoc_vcl_with_inline_c_nests_both_injections() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = concat!(
            "my $vcl = <<VCL;\n",
            "sub vcl_recv {\n",
            "  C{ static int hits = 0; }C\n",
            "  return (pass);\n",
            "}\n",
            "VCL\n",
            "print $vcl;\n",
        );
        let spans = highlight(src, lang);
        let at = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(at("sub"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(
            at("return"),
            Some(("keyword.control.return".to_string(), "vcl"))
        );
        assert_eq!(at("C{"), Some(("punctuation.special".to_string(), "vcl")));
        assert_eq!(at("static"), Some(("keyword".to_string(), "c")));
        assert_eq!(at("int"), Some(("type".to_string(), "c")));
        assert_eq!(at("}C"), Some(("punctuation.special".to_string(), "vcl")));
        // The surrounding Perl is untouched.
        assert_eq!(at("print").map(|(_, l)| l), Some("perl"));
    }

    #[test]
    fn heredoc_spans_carry_the_injected_language() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "my $x = 1;\nprint <<SQL;\n   SELECT 1\nSQL\n";
        let spans = highlight(src, lang);
        let select = src.find("SELECT").unwrap();
        let sql_span = spans
            .iter()
            .find(|s| s.start == select)
            .expect("SELECT should be highlighted");
        assert_eq!(sql_span.language, "sql", "{sql_span:?}");
        // Everything outside the heredoc body is still Perl's, including
        // the numeric-fallback span for `1`.
        let one = src.find('1').unwrap();
        assert!(
            spans.iter().any(|s| s.start == one && s.language == "perl"),
            "{spans:?}"
        );
        assert!(
            spans
                .iter()
                .filter(|s| s.language == "sql")
                .all(|s| s.start >= src.find("   SELECT").unwrap()),
            "no sql-tagged span may leak outside the heredoc body: {spans:?}"
        );
    }

    /// Bash, Ruby and PHP heredocs inject the same way Perl's do, through
    /// the grammar's own body nodes: including Bash's `<<-`, Ruby's
    /// indented `<<~` and `<<-`, PHP's nowdoc, and quoted terminators.
    #[test]
    fn heredocs_inject_in_bash_ruby_and_php() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "bash",
                "a.sh",
                "psql <<SQL\nSELECT * FROM foo WHERE bar = 1\nSQL\necho done\n",
            ),
            (
                "bash",
                "a.sh",
                "psql <<-'SQL'\n\tSELECT * FROM foo\n\tSQL\n",
            ),
            (
                "ruby",
                "a.rb",
                "q = <<~SQL\n  SELECT * FROM foo\n  WHERE bar = 1\nSQL\nputs q\n",
            ),
            (
                "ruby",
                "a.rb",
                "q = <<-'SQL'\n  SELECT * FROM foo\n  SQL\nputs q\n",
            ),
            (
                "ruby",
                "a.rb",
                "h = { a: <<~SQL, b: 1 }\n  SELECT * FROM foo\nSQL\n",
            ),
            (
                "php",
                "a.php",
                "<?php\n$q = <<<SQL\nSELECT * FROM foo\nSQL;\necho $q;\n",
            ),
            (
                "php",
                "a.php",
                "<?php\n$q = <<<'SQL'\nSELECT * FROM foo\nSQL;\n",
            ),
        ];
        for (outer, path, src) in cases {
            let lang = languages::detect(Some(std::path::Path::new(path)), src).unwrap();
            assert_eq!(lang.name, *outer, "{path}");
            let spans = highlight(src, lang);
            let select = src.find("SELECT").unwrap();
            assert!(
                spans.iter().any(|s| s.language == "sql"
                    && s.start == select
                    && s.end == select + 6
                    && s.scope.name().starts_with("keyword")),
                "{outer} {src:?}: no SQL keyword span for SELECT, got {spans:?}"
            );
            // Nothing of the outer language survives inside the body.
            let body_end = src.rfind("SQL").unwrap();
            assert!(
                spans
                    .iter()
                    .filter(|s| s.start >= select && s.end <= body_end)
                    .all(|s| s.language == "sql"),
                "{outer} {src:?}: outer spans leaked into the heredoc body"
            );
            // Code after the terminator is still the outer language.
            if let Some(after) = src.find("puts").or_else(|| src.find("echo")) {
                assert!(
                    spans
                        .iter()
                        .any(|s| s.start == after && s.language == *outer),
                    "{outer} {src:?}: code after the heredoc lost its language"
                );
            }
        }
    }

    #[test]
    fn heredoc_unknown_terminator_stays_plain_string() {
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), "").unwrap();
        let src = "print <<EOF;\nsome text\nEOF\n";
        let spans = highlight(src, lang);
        // "EOF" isn't a recognized language name, so the body should still
        // be covered by the outer Perl query's plain @string capture.
        let body_start = src.find("some text").unwrap();
        assert!(
            spans.iter().any(|s| s.scope.name().starts_with("string")
                && s.start <= body_start
                && s.end >= body_start + 9),
            "expected the heredoc body to remain a string span, got {spans:?}"
        );
    }

    /// A heredoc inside a multi-line construct (here a hash argument, as
    /// in a `.t` file) must not derail the Perl parse of everything after
    /// it. (The grammar tico used before parsed such a body as Perl code,
    /// then swallowed the rest of the file as heredoc content once the
    /// enclosing statement ended.)
    #[test]
    fn heredoc_in_multiline_construct_keeps_following_perl_intact() {
        let src = concat!(
            "$client->send(\n",
            "    'snippet.create',\n",
            "    {\n",
            "        content => <<~'VCL',\n",
            "            set req.http.Key = \"AKIA;EXAMPLE\";\n",
            "            set req.http.Token = \"xoxb-1\";\n",
            "            VCL\n",
            "    }\n",
            ")->status_code_is(200);\n",
            "is(\n",
            "    $vclsearch->send( 'vclsearch.show', { service_id => $service->id } ),\n",
            "    { error => 'service not found' },\n",
            "    'show returns 404',\n",
            ");\n",
            "$service->activate_ok(1);\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.t")), src).unwrap();
        let spans = highlight(src, lang);
        let exact = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        // The body is VCL; the marker and the terminator are labels, as the
        // Perl query makes them.
        assert_eq!(exact("set"), Some(("keyword".to_string(), "vcl")));
        assert_eq!(exact("<<~'VCL'"), Some(("label".to_string(), "perl")));
        let terminator = src.rfind("VCL").unwrap();
        assert!(
            spans.iter().any(|s| s.start == terminator
                && s.end == terminator + 3
                && s.scope.name() == "label"
                && s.language == "perl"),
            "terminator should be a Perl label span: {spans:?}"
        );
        // Everything after the heredoc is ordinary Perl again.
        assert_eq!(
            exact("'vclsearch.show'"),
            Some(("string".to_string(), "perl"))
        );
        assert_eq!(
            exact("'service not found'"),
            Some(("string".to_string(), "perl"))
        );
        assert_eq!(
            exact("'show returns 404'"),
            Some(("string".to_string(), "perl"))
        );
        assert_eq!(
            exact("$vclsearch"),
            Some(("variable.scalar".to_string(), "perl"))
        );
        assert_eq!(
            exact("$service"),
            Some(("variable.scalar".to_string(), "perl"))
        );
        let one = src.rfind('1').unwrap();
        assert!(
            spans.iter().any(|s| s.start == one
                && s.end == one + 1
                && s.scope.name() == "constant.numeric"
                && s.language == "perl"),
            "activate_ok(1) should keep its numeric span: {spans:?}"
        );
        // No Perl span reaches into or across the heredoc.
        let (body_start, body_end) = (src.find("            set").unwrap(), terminator);
        assert!(
            spans
                .iter()
                .filter(|s| s.language == "perl")
                .all(|s| s.end <= body_start || s.start >= body_end),
            "a Perl span overlaps the heredoc body: {spans:?}"
        );
    }

    /// Several heredocs in multi-line constructs, one after another, are
    /// each injected, with the Perl between them intact.
    #[test]
    fn heredocs_hidden_behind_an_earlier_one_are_still_injected() {
        let one = concat!(
            "my $h = {\n",
            "    content => <<~'VCL',\n",
            "        set req.http.Key = \"a;b\";\n",
            "        VCL\n",
            "    other => 'x',\n",
            "};\n",
        );
        let src = one.repeat(3);
        let lang = languages::detect(Some(std::path::Path::new("a.t")), &src).unwrap();
        let spans = highlight(&src, lang);
        let mut from = 0;
        for _ in 0..3 {
            let set = src[from..].find("set").unwrap() + from;
            assert!(
                spans.iter().any(|s| s.start == set && s.language == "vcl"),
                "`set` at {set} should be VCL: {spans:?}"
            );
            let x = src[from..].find("'x'").unwrap() + from;
            assert!(
                spans.iter().any(|s| s.start == x
                    && s.end == x + 3
                    && s.scope.name() == "string"
                    && s.language == "perl"),
                "'x' at {x} should be a Perl string: {spans:?}"
            );
            from = x;
        }
    }

    /// A heredoc naming no language: a `<<~EOT` body in a hash reads as
    /// one string (through its terminator, which the grammar's body node
    /// includes), and doesn't disturb the code after it.
    #[test]
    fn unknown_heredoc_in_multiline_construct_is_a_string() {
        let src = concat!(
            "my $h = {\n",
            "    text => <<~EOT,\n",
            "        it's \"quoted\"; sort of\n",
            "        EOT\n",
            "    other => 'x',\n",
            "};\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), src).unwrap();
        let spans = highlight(src, lang);
        let body_start = src.find("        it's").unwrap();
        let body_end = src.find("        EOT").unwrap() + "        EOT".len();
        assert!(
            spans.iter().any(|s| s.start == body_start
                && s.end == body_end
                && s.scope.name() == "string"
                && s.language == "perl"),
            "expected one string span over the body: {spans:?}"
        );
        let x = src.find("'x'").unwrap();
        assert!(
            spans
                .iter()
                .any(|s| s.start == x && s.end == x + 3 && s.scope.name() == "string"),
            "'x' should be a string: {spans:?}"
        );
    }

    /// A `__DATA__` section laid out as `@@ name` parts (Mojo::Loader,
    /// Data::Section::Simple, Data::Section::Pluggable): each part is
    /// highlighted as the file its name suggests, a `(base64)` part and a
    /// part with an unknown extension stay plain, and the markers on the
    /// `@@` lines are colored.
    #[test]
    fn data_section_parts_are_highlighted_by_name() {
        let src = concat!(
            "print \"hi\";\n",
            "__DATA__\n",
            "\n",
            "@@ hello.txt\n",
            "  Welcome to Perl\n",
            "\n",
            "@@ hello.json\n",
            "{\"message\":\"Welcome to Perl\"}\n",
            "\n",
            "@@ hello.bin (base64)\n",
            "VGhpcyBpcyBiYXNlNjQgZW5jb2RlZC4K\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), src).unwrap();
        let spans = highlight(src, lang);
        let exact = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(
            exact("__DATA__"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(
            exact("@@"),
            Some(("punctuation.special".to_string(), "perl"))
        );
        assert_eq!(
            exact("hello.txt"),
            Some(("string.special.path".to_string(), "perl"))
        );
        assert_eq!(
            exact("hello.bin"),
            Some(("string.special.path".to_string(), "perl"))
        );
        assert_eq!(
            exact("(base64)"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(exact("\"message\""), Some(("string".to_string(), "json")));

        // JSON spans stay inside the JSON part; the text and base64 parts
        // get no spans at all beyond their header lines.
        let json_body = src.find("{\"message\"").unwrap();
        let json_end = src.find("@@ hello.bin").unwrap();
        assert!(
            spans
                .iter()
                .filter(|s| s.language == "json")
                .all(|s| s.start >= json_body && s.end <= json_end),
            "json span outside its part: {spans:?}"
        );
        for body in ["  Welcome to Perl", "VGhpcyBpcyBiYXNlNjQgZW5jb2RlZC4K"] {
            let start = src.find(body).unwrap();
            let end = start + body.len();
            assert!(
                !spans.iter().any(|s| s.start < end && s.end > start),
                "{body:?} should be plain: {spans:?}"
            );
        }
    }

    /// `__END__` opens a data section too, text before the first `@@` is
    /// plain, and an `__END__` line inside the section ends it, as it does
    /// for Data::Section::Pluggable. The other special literals are
    /// colored as the compile-time constants they are.
    #[test]
    fn data_section_after_end_marker_stops_at_inner_end_marker() {
        let src = concat!(
            "my $f = __FILE__;\n",
            "__END__\n",
            "ignored 'text'\n",
            "@@ a.sql\n",
            "SELECT 1;\n",
            "__END__\n",
            "@@ c.json\n",
            "{\"x\":1}\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), src).unwrap();
        let spans = highlight(src, lang);
        let exact = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        assert_eq!(
            exact("__FILE__"),
            Some(("constant.builtin".to_string(), "perl"))
        );
        assert_eq!(
            exact("__END__"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(exact("SELECT"), Some(("keyword".to_string(), "sql")));
        assert_eq!(exact("'text'"), None);
        let after = src.find("@@ c.json").unwrap();
        assert!(
            !spans.iter().any(|s| s.start >= after),
            "nothing after the inner __END__ is part of the data: {spans:?}"
        );
    }

    /// `use Inline LANG => ...` names the language of the source that
    /// follows the `=>`, whatever form it takes: a heredoc (whose
    /// terminator, `END` here, names no language by itself), a `q{}`
    /// string, or code in `__DATA__` under Inline's `__LANG__` marker.
    #[test]
    fn inline_source_is_highlighted_in_its_language() {
        let src = concat!(
            "use Inline C => <<'END';\n",
            "int f() { return 1; }\n",
            "END\n",
            "use Inline C => q{ int g(void); }, libs => \"-lm\";\n",
            "use Inline C => 'DATA';\n",
            "use Inline Python => <<END;\n",
            "def p(): pass\n",
            "END\n",
            "my $x = <<'END';\n",
            "int not_c() {}\n",
            "END\n",
            "__DATA__\n",
            "__C__\n",
            "int h() { return 2; }\n",
            "__Python__\n",
            "def q(): pass\n",
            "__CPP__\n",
            "namespace n { int y = 0; }\n",
        );
        let lang = languages::detect(Some(std::path::Path::new("a.pl")), src).unwrap();
        let spans = highlight(src, lang);
        let exact = |needle: &str| {
            let start = src.find(needle).unwrap();
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        let at = |needle: &str, n: usize| {
            let start = src.match_indices(needle).nth(n).unwrap().0;
            spans
                .iter()
                .filter(|s| s.start == start && s.end == start + needle.len())
                .map(|s| (s.scope.name().to_string(), s.language))
                .next_back()
        };
        // The heredoc after `C =>`.
        assert_eq!(at("int", 0), Some(("type".to_string(), "c")));
        assert_eq!(at("return", 0), Some(("keyword".to_string(), "c")));
        // The q{} string; the `libs` option after it is still Perl.
        assert_eq!(exact("void"), Some(("type".to_string(), "c")));
        assert_eq!(exact("libs"), Some(("string.special".to_string(), "perl")));
        assert_eq!(exact("\"-lm\""), Some(("string".to_string(), "perl")));
        // 'DATA' is a one-line string: not source.
        assert_eq!(exact("'DATA'"), Some(("string".to_string(), "perl")));
        // A different Inline language, and a heredoc that isn't Inline's
        // at all (its terminator names no language): one Perl string.
        assert_eq!(at("def", 0), Some(("keyword".to_string(), "python")));
        let not_c = src.find("int not_c").unwrap();
        assert!(
            spans
                .iter()
                .any(|s| s.start == not_c && s.scope.name() == "string" && s.language == "perl"),
            "a plain <<'END' heredoc stays a Perl string: {spans:?}"
        );
        assert!(
            !spans.iter().any(|s| s.language == "c" && s.start == not_c),
            "{spans:?}"
        );
        // The __DATA__ markers and the parts under them.
        assert_eq!(
            exact("__C__"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(
            exact("__Python__"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(at("return", 1), Some(("keyword".to_string(), "c")));
        assert_eq!(at("def", 1), Some(("keyword".to_string(), "python")));
        let python_part = src.find("__Python__").unwrap();
        assert!(
            spans
                .iter()
                .filter(|s| s.language == "c")
                .all(|s| s.end <= python_part),
            "C spans stop at the __Python__ marker: {spans:?}"
        );
        // Inline's `CPP` is tico's `cpp`, matched case-insensitively.
        assert_eq!(
            exact("__CPP__"),
            Some(("keyword.directive".to_string(), "perl"))
        );
        assert_eq!(exact("namespace"), Some(("keyword".to_string(), "cpp")));
    }

    #[test]
    fn override_forces_language_regardless_of_filename() {
        let lang =
            detect_with_override(Some(std::path::Path::new("foo.txt")), "", Some("perl")).unwrap();
        assert_eq!(lang.name, "perl");
    }

    #[test]
    fn override_none_disables_detection() {
        assert!(
            detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("none")).is_none()
        );
    }

    #[test]
    fn override_unknown_name_falls_back_to_detection() {
        let lang =
            detect_with_override(Some(std::path::Path::new("foo.pl")), "", Some("boguslang"))
                .unwrap();
        assert_eq!(lang.name, "perl");
    }

    #[test]
    fn listed_names_are_sorted_and_nonempty() {
        let names = names();
        assert!(names.contains(&"perl"));
        assert!(names.contains(&"sql"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn perl_extensions() {
        for ext in ["pl", "pm", "t", "xs"] {
            let path = format!("foo.{ext}");
            let lang = detect(Some(std::path::Path::new(&path)), "")
                .expect("perl file should be detected");
            assert_eq!(lang.name, "perl");
        }
    }

    /// A file whose name says nothing (`foo.conf`, no name at all) is
    /// sniffed for JSON's opening bytes, nano-`header` style; a
    /// recognized name always wins over the sniff.
    #[test]
    fn json_content_sniffing() {
        let conf = Some(std::path::Path::new("app.conf"));
        let json_texts = [
            "{\n  \"key\": 1\n}\n",
            "{\"a\":1}",
            "{}",
            "  \n\t{ \"x\": [] }",
            "\u{feff}{\"bom\": true}",
            "[{\"a\": 1}]",
            "[[1, 2], [3]]",
            "[\"a\", \"b\"]",
            "[1, 2, 3]",
            "[-1]",
            "[]",
            "[true, false, null]",
            "// jsonc comment\n{\"a\": 1}",
            "/* block\n comment */ // line\n[1]",
        ];
        for text in json_texts {
            for path in [conf, None] {
                let lang = detect(path, text)
                    .unwrap_or_else(|| panic!("{text:?} should be detected as json"));
                assert_eq!(lang.name, "json", "{text:?}");
            }
        }
        let not_json = [
            "",
            "   \n",
            "# comment\n{\"a\": 1}",
            "[section]\nkey = value\n",
            "[ section ]",
            "{ foo => 1 }",
            "{\n  int x;\n}",
            "{",
            "server {\n  listen 80;\n}\n",
            "key = value\n",
            "<VirtualHost *:80>\n",
            "/* unterminated",
            "// only a comment\n",
            "42",
            "\"a string\"",
        ];
        for text in not_json {
            assert!(
                detect(conf, text).is_none(),
                "{text:?} should not be detected as json"
            );
        }
        // The filename wins: JSON content in an ini-named file is ini, and
        // a `.json` extension is json regardless of what's inside.
        assert_eq!(
            detect(Some(std::path::Path::new("a.ini")), "{\"a\": 1}")
                .unwrap()
                .name,
            "ini"
        );
        assert_eq!(
            detect(Some(std::path::Path::new("a.json")), "[section]")
                .unwrap()
                .name,
            "json"
        );
        // So does a modeline.
        assert_eq!(
            detect(conf, "{\"a\": 1}\n# vim: ft=yaml\n").unwrap().name,
            "yaml"
        );
    }

    /// `.inc` is not registered as a Pascal extension (assembler, PHP and
    /// C includes use it too), so a `.inc` file is sniffed: it's Pascal
    /// when its first non-blank line opens a compiler directive, a
    /// comment, a section header or a routine. No other filename is.
    #[test]
    fn pascal_inc_content_sniffing() {
        let inc = Some(std::path::Path::new("defines.inc"));
        let pascal_texts = [
            "{$mode objfpc}\n",
            "  {$ifdef FPC}\nconst X = 1;\n{$endif}\n",
            "{$I-}",
            "\n\n{$IFDEF VER70}\n",
            "(* block comment *)\nvar\n  I: Integer;\n",
            "{ brace comment }\n",
            "{comment}\n",
            "unit Foo;\n",
            "Program Foo;",
            "uses Crt, Dos;",
            "interface\n",
            "implementation\n",
            "procedure Foo;\nbegin\nend;\n",
            "function Foo: Integer;\n",
            "function Foo(A: Integer): Integer; far;\n",
            "constructor TFoo.Init;\n",
            "destructor TFoo.Done;\n",
            "const\n  X = 1;\n",
            "type\n  T = Integer;\n",
            "var\n  I: Integer;\n",
            "  Var\n",
            "label 10;\n",
            "resourcestring\n  S = 'x';\n",
        ];
        for text in pascal_texts {
            assert_eq!(
                detect(inc, text).map(|l| l.name),
                Some("pascal"),
                "{text:?} should be detected as pascal"
            );
            // The sniff is specific to `.inc`: the same content under any
            // other unregistered name, or no name, is not sniffed.
            for path in [Some(std::path::Path::new("app.conf")), None] {
                assert!(
                    detect(path, text).is_none(),
                    "{text:?} should only be sniffed as pascal in a .inc file"
                );
            }
        }
        let not_pascal = [
            "",
            "   \n",
            "<?php\nfunction foo() {}\n",
            "<?\n",
            "; nasm include\n%macro X 1\n",
            "section .text\n",
            "#include <stdio.h>\n",
            "#ifndef FOO_H\n",
            "# shell fragment\nFOO=1\n",
            "const x = 1;\n",
            "type Foo struct {}\n",
            "interface Foo {\n",
            "function foo() {\n",
            "var x = 1;\n",
            "server {\n  listen 80;\n}\n",
            "<Directory />\n",
            "{\n  int x;\n}\n",
            "{\"a\": 1}",
            "---\nkey: value\n",
            "begin\n",
            "WriteLn('x');\n",
        ];
        for text in not_pascal {
            assert_ne!(
                detect(inc, text).map(|l| l.name),
                Some("pascal"),
                "{text:?} should not be detected as pascal"
            );
        }
        // Registered extensions need no sniffing, whatever the content
        // (and DOS-era files are often upper-case).
        for name in ["x.pas", "X.PAS", "x.pp", "x.dpr", "x.lpr", "x.dpk"] {
            assert_eq!(
                detect(Some(std::path::Path::new(name)), "").unwrap().name,
                "pascal",
                "{name}"
            );
        }
        assert!(detect(Some(std::path::Path::new("x.inc")), "").is_none());
    }

    /// nano's yaml `header` rule (`^%YAML |^---( |$)`) on the first
    /// non-blank, non-comment line; a unified diff's `--- `/`+++ ` pair is
    /// told apart and detected as diff instead.
    #[test]
    fn yaml_and_diff_content_sniffing() {
        let conf = Some(std::path::Path::new("app.conf"));
        let yaml_texts = [
            "---\n",
            "---",
            "---\nkey: value\n",
            "--- # doc\nkey: value\n",
            "--- !tag\n",
            "%YAML 1.2\n---\n",
            "# comment\n\n# more\n---\nkey: value\n",
            "\u{feff}---\nkey: value\n",
            "--- a/file\n",
        ];
        for text in yaml_texts {
            for path in [conf, None] {
                let lang = detect(path, text)
                    .unwrap_or_else(|| panic!("{text:?} should be detected as yaml"));
                assert_eq!(lang.name, "yaml", "{text:?}");
            }
        }
        let diff_texts = [
            "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-x\n+y\n",
            "--- old.txt\t2026-01-01\n+++ new.txt\t2026-01-02\n",
        ];
        for text in diff_texts {
            for path in [conf, None] {
                let lang = detect(path, text)
                    .unwrap_or_else(|| panic!("{text:?} should be detected as diff"));
                assert_eq!(lang.name, "diff", "{text:?}");
            }
        }
        let neither = [
            "----\n",
            "---x\n",
            " ---\n",
            "key: value\n",
            "# just a comment\n",
            "%YAML\n",
            "%YAMLX 1.2\n",
            "-- sql comment\n",
            "+++ b/file\n--- a/file\n",
        ];
        for text in neither {
            assert!(
                detect(conf, text).is_none(),
                "{text:?} should not be detected"
            );
        }
        // Filename still wins.
        assert_eq!(
            detect(Some(std::path::Path::new("a.ini")), "---\n")
                .unwrap()
                .name,
            "ini"
        );
        // A JSON opening beats a later YAML marker check, since it's
        // looked at first.
        assert_eq!(detect(conf, "{\"a\": 1}\n---\n").unwrap().name, "json");
    }

    #[test]
    fn filename_prefix_patterns() {
        let cases = [
            ("cpanfile", "perl"),
            ("lib/cpanfile.dev", "perl"),
            ("Makefile.PL", "perl"),
            ("Makefile.in", "make"),
            ("Makefile.am", "make"),
            ("src/Makefile.inc", "make"),
            ("makefile.unix", "make"),
            ("GNUmakefile.local", "make"),
            ("app.properties", "properties"),
            ("org.eclipse.core.prefs", "properties"),
            ("log4perl.conf", "properties"),
            ("log4perl.debug.conf", "properties"),
            ("etc/log4perl.prod.conf", "properties"),
            ("log4j.properties", "properties"),
            ("Dockerfile", "dockerfile"),
            ("dockerfile", "dockerfile"),
            ("Dockerfile.dev", "dockerfile"),
            ("docker/Dockerfile.alpine", "dockerfile"),
            ("Containerfile", "dockerfile"),
            ("Containerfile.build", "dockerfile"),
            ("app.dockerfile", "dockerfile"),
            ("app.Dockerfile", "dockerfile"),
        ];
        for (path, expected) in cases {
            let lang = detect(Some(std::path::Path::new(path)), "")
                .unwrap_or_else(|| panic!("{path} should be detected"));
            assert_eq!(lang.name, expected, "{path}");
        }
        // Only `NAME.` prefixes match: a bare prefix without the dot, or a
        // different file that merely contains the name, stays plain.
        // `cpanfile.snapshot` is Carton's lockfile, not Perl.
        for path in [
            "Makefiles",
            "cpanfiles",
            "notcpanfile.x",
            "MyMakefile.in",
            "cpanfile.snapshot",
            "dir/cpanfile.snapshot",
            "log4perl.conf.bak",
            "mylog4perl.x.conf",
            "log4perlconf",
            "Dockerfiles",
            "MyDockerfile",
        ] {
            assert!(
                detect(Some(std::path::Path::new(path)), "").is_none(),
                "{path} should not be detected"
            );
        }
    }

    #[test]
    fn all_languages_query_compiles_and_highlights() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "c",
                "a.c",
                "#include <stdio.h>\nint main() { return 0; } // hi\n",
            ),
            (
                "cpp",
                "a.cpp",
                "#include <iostream>\nclass Foo { public: int x; }; // hi\n",
            ),
            ("rust", "a.rs", "fn main() { let x = 1; } // hi\n"),
            ("go", "a.go", "package main\nfunc main() { x := 1 } // hi\n"),
            (
                "python",
                "a.py",
                "def foo():\n    x = 1  # hi\n    return x\n",
            ),
            (
                "bash",
                "a.sh",
                "#!/bin/bash\nfoo() { echo hi; } # comment\n",
            ),
            ("json", "a.json", "{\"a\": 1, \"b\": \"c\"}\n"),
            ("yaml", "a.yaml", "a: 1\nb: \"c\" # hi\n"),
            ("toml", "a.toml", "a = 1\nb = \"c\" # hi\n"),
            ("html", "a.html", "<html><!-- hi --><body>x</body></html>\n"),
            ("css", "a.css", "/* hi */ .a { color: red; }\n"),
            ("sql", "a.sql", "SELECT * FROM foo WHERE x = 1; -- hi\n"),
            ("javascript", "a.js", "function foo() { return 1; } // hi\n"),
            (
                "typescript",
                "a.ts",
                "function foo(): number { return 1; } // hi\n",
            ),
            ("java", "a.java", "class Foo { void bar() {} } // hi\n"),
            ("ruby", "a.rb", "def foo\n  1 # hi\nend\n"),
            (
                "php",
                "a.php",
                "<?php\nfunction foo() { return 1; } // hi\n",
            ),
            ("csharp", "a.cs", "class Foo { void Bar() {} } // hi\n"),
            ("make", "Makefile", "all:\n\techo hi # comment\n"),
            (
                "fortran",
                "a.f90",
                "program hi\n  integer :: x = 1\nend program hi\n",
            ),
            ("markdown", "a.md", "# Title\n\nSome *text*.\n"),
            ("toml", "Cargo.toml", "[package]\nname = \"x\"\n"),
            ("haskell", "a.hs", "main = putStrLn \"hi\" -- comment\n"),
            ("scala", "a.scala", "object Foo { def bar() = 1 } // hi\n"),
            ("objc", "a.m", "@interface Foo : NSObject @end // hi\n"),
            ("r", "a.r", "foo <- function(x) { x + 1 } # hi\n"),
            ("julia", "a.jl", "function foo(x)\n    x + 1 # hi\nend\n"),
            ("xml", "a.xml", "<!-- hi --><root><a>1</a></root>\n"),
            ("diff", "a.diff", "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n"),
            ("ini", "a.ini", "[section]\n; comment\nkey = value\n"),
            (
                "elixir",
                "a.ex",
                "defmodule Foo do\n  def bar, do: 1 # hi\nend\n",
            ),
            ("elm", "a.elm", "foo x = x + 1 -- hi\n"),
            ("zig", "a.zig", "pub fn main() void { } // hi\n"),
            ("dart", "a.dart", "void main() { print('hi'); } // hi\n"),
            ("scss", "a.scss", "// hi\n.a { color: red; }\n"),
            (
                "proto",
                "a.proto",
                "// hi\nmessage Foo { string bar = 1; }\n",
            ),
            (
                "cmake",
                "CMakeLists.txt",
                "# hi\nadd_executable(foo bar.c)\n",
            ),
            ("nix", "a.nix", "# hi\n{ foo = 1; }\n"),
            ("vim", "a.vim", "\" hi\nlet g:foo = 1\n"),
            ("lua", "a.lua", "-- hi\nfunction foo() return 1 end\n"),
            ("swift", "a.swift", "func foo() -> Int { return 1 } // hi\n"),
            ("go", "a.go", "package main\n// hi\nfunc main() {}\n"),
            (
                "vcl",
                "default.vcl",
                "vcl 4.1;\nsub vcl_recv { # hi\n    return (pass);\n}\n",
            ),
            (
                "groovy",
                "a.groovy",
                "class Foo {\n    def bar() { return 1 } // hi\n}\n",
            ),
            (
                "tcl",
                "a.tcl",
                "#!/usr/bin/env tclsh\nproc greet {name} { puts \"hi $name\" } ;# hi\nset x [expr {1 + 2}]\n",
            ),
            (
                "dockerfile",
                "Dockerfile",
                "# hi\nFROM alpine:3.20 AS base\nARG VERSION=1\nRUN echo $VERSION\nEXPOSE 80\n",
            ),
            (
                "tt2",
                "default.vcl.tt",
                "sub vcl_recv {\n[% IF debug %]  set req.http.X = \"[% name %]\";\n[% END %]}\n",
            ),
            (
                "powershell",
                "a.ps1",
                "# hi\nfunction Foo { param($x) Write-Output $x }\n",
            ),
            ("batch", "a.cmd", "@echo off\nREM hi\nset X=1\necho %X%\n"),
            (
                "v",
                "a.v",
                "module main\n// hi\nfn main() {\n\tx := 1\n\tprintln('${x}')\n}\n",
            ),
            (
                "cue",
                "a.cue",
                "package p\n// hi\n#S: { name: string, n: int | *1 }\nx: #S & { name: \"a\" }\n",
            ),
            (
                "hcl",
                "main.tf",
                "# hi\nresource \"aws_instance\" \"web\" {\n  ami = var.ami\n  count = 2\n}\n",
            ),
            (
                "properties",
                "log4perl.conf",
                "# hi\nlog4perl.rootLogger=INFO, Screen\nlog4perl.appender.Screen.layout.ConversionPattern=[%5p] %m%n\n",
            ),
        ];
        for (name, path, source) in cases {
            check(name, path, source, true);
        }
    }

    /// Every capture name every vendored query produces must, after
    /// `normalize_capture`, start with one of the top-level scope names
    /// Helix themes are written against -- otherwise no theme could ever
    /// style it, and a new query (or a new upstream revision of one) that
    /// brings a novel nvim-ism would silently render uncolored.
    #[test]
    fn all_captures_normalize_into_helix_scopes() {
        const HELIX_TOP_LEVEL: &[&str] = &[
            "attribute",
            "type",
            "constructor",
            "constant",
            "string",
            "comment",
            "variable",
            "label",
            "punctuation",
            "keyword",
            "operator",
            "function",
            "tag",
            "namespace",
            "special",
            "markup",
            "diff",
        ];
        let mut bad = Vec::new();
        for name in names() {
            let lang = find_by_name(name).unwrap();
            let query = tree_sitter::Query::new(&(lang.language)(), lang.highlights_query)
                .unwrap_or_else(|e| panic!("{name}: query failed to compile: {e}"));
            for raw in query.capture_names() {
                let Some(scope) = normalize_capture(raw) else {
                    continue;
                };
                let head = scope.split('.').next().unwrap();
                if !HELIX_TOP_LEVEL.contains(&head) {
                    bad.push(format!("{name}: @{raw} -> {scope}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "captures outside Helix's scope vocabulary:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn normalize_capture_renames_nvim_conventions() {
        let n = |s: &str| normalize_capture(s);
        assert_eq!(n("number").as_deref(), Some("constant.numeric"));
        assert_eq!(n("float").as_deref(), Some("constant.numeric.float"));
        assert_eq!(
            n("conditional").as_deref(),
            Some("keyword.control.conditional")
        );
        assert_eq!(
            n("keyword.import").as_deref(),
            Some("keyword.control.import")
        );
        assert_eq!(n("property").as_deref(), Some("variable.other.member"));
        assert_eq!(n("text.title").as_deref(), Some("markup.heading"));
        assert_eq!(n("string.elm").as_deref(), Some("string"));
        assert_eq!(n("function.elm").as_deref(), Some("function"));
        // Already-Helix names pass through untouched, tail included.
        assert_eq!(
            n("keyword.control.import").as_deref(),
            Some("keyword.control.import")
        );
        assert_eq!(n("variable.builtin").as_deref(), Some("variable.builtin"));
        assert_eq!(
            n("punctuation.section.braces").as_deref(),
            Some("punctuation.section.braces")
        );
        // Head-only renames keep their tail.
        assert_eq!(
            n("property.foo").as_deref(),
            Some("variable.other.member.foo")
        );
        // Dropped outright.
        assert_eq!(n("_name"), None);
        assert_eq!(n("spell"), None);
        assert_eq!(n("source.glsl"), None);
        assert_eq!(n("text.warning"), None);
    }

    #[test]
    fn scopes_intern_to_stable_ids() {
        let a = Scope::intern("keyword.control");
        let b = Scope::intern("keyword.control");
        let c = Scope::intern("keyword");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.name(), "keyword.control");
        assert_eq!(c.name(), "keyword");
    }
}
