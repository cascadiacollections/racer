//! Codes on the clipboard (DESIGN §9.11): what COPY CODE puts there and
//! what PASTE CODE finds in whatever text comes back.
//!
//! A code survives a trip through a chat or a mail: wrapped across lines,
//! indented, quoted, hyphenated, lower-cased, with words around it. So a
//! paste is searched rather than parsed: every place an `RL` or `RD`
//! starts (any case) begins a candidate, which runs on over base32
//! characters, spaces, tabs, hyphens and single line breaks, stopping at a
//! blank line or a line that starts `origin` or `#`; the first candidate
//! that decodes wins. An `origin` line after it (a layout file) comes with
//! it. Text that holds no code says why the most promising candidate
//! failed.

use racer_sim::layout::{CodeError, Layout, CODE_PREFIXES, MAX_CODE_CHARS};

/// What a paste found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pasted {
    /// The layout.
    pub layout: Layout,
    /// The `origin` line that came with it, if any.
    pub origin: Option<String>,
}

/// Whether `c` can be part of a code once spaces are dropped.
fn code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() && !matches!(c.to_ascii_uppercase(), 'U')
}

/// The candidate starting at byte `at`: its characters with spaces,
/// hyphens and line breaks dropped, where it ended, and for each break in
/// it (where words around a code could begin) the length so far and the
/// byte it ended at.
fn candidate(text: &str, at: usize) -> (String, usize, Vec<(usize, usize)>) {
    let mut cuts = Vec::new();
    let mut out = String::new();
    let mut end = at;
    let mut newline = false;
    let rest = &text[at..];
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek().is_some_and(|(_, n)| *n == '\n') {
                chars.next();
            }
            if newline {
                break; // a blank line ends it
            }
            newline = true;
            if cuts
                .last()
                .is_none_or(|c: &(usize, usize)| c.0 != out.len())
            {
                cuts.push((out.len(), end));
            }
            // A new line that starts a comment or an origin ends it.
            let line = rest[i + 1..].trim_start_matches(['\n', '\r']).trim_start();
            if line.starts_with('#') || line.to_ascii_lowercase().starts_with("origin") {
                break;
            }
            continue;
        }
        if c == ' ' || c == '\t' || c == '-' || c == '>' {
            // Spaces, hyphens and quote marks inside a code are dropped.
            if cuts
                .last()
                .is_none_or(|c: &(usize, usize)| c.0 != out.len())
            {
                cuts.push((out.len(), end));
            }
            continue;
        }
        if !code_char(c) {
            break;
        }
        newline = false;
        out.push(c);
        end = at + i + c.len_utf8();
        if out.len() > MAX_CODE_CHARS {
            break;
        }
    }
    (out, end, cuts)
}

/// The `origin` line after byte `from`, if the next non-blank line is one.
fn origin_after(text: &str, from: usize) -> Option<String> {
    let line = text[from..]
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    let o = line.strip_prefix("origin ")?.trim();
    (!o.is_empty() && !o.contains(char::is_whitespace)).then(|| o.to_string())
}

/// Find a layout in pasted text.
pub fn paste(text: &str) -> Result<Pasted, CodeError> {
    if text.len() > 4 * MAX_CODE_CHARS {
        return Err(CodeError::TooLong);
    }
    let upper = text.to_ascii_uppercase();
    let mut best: Option<(usize, CodeError)> = None;
    let mut from = 0;
    while let Some(off) = CODE_PREFIXES
        .iter()
        .filter_map(|p| upper[from..].find(p))
        .min()
    {
        let at = from + off;
        from = at + 1;
        // A prefix inside a word ("HARD", "WORLD") is not a code's start.
        if at > 0 && text[..at].chars().next_back().is_some_and(code_char) {
            continue;
        }
        let (code, end, cuts) = candidate(text, at);
        // The whole run first, then shorter at each break, longest first:
        // words written after a code are not part of it.
        let tries = std::iter::once((code.len(), end)).chain(cuts.into_iter().rev());
        for (len, end) in tries {
            if len <= 2 {
                continue;
            }
            match Layout::from_code(&code[..len]) {
                Ok(layout) => {
                    return Ok(Pasted {
                        layout,
                        origin: origin_after(text, end),
                    })
                }
                Err(e) => {
                    if best.as_ref().is_none_or(|(l, _)| len > *l) {
                        best = Some((len, e));
                    }
                }
            }
        }
    }
    Err(best.map_or(CodeError::Prefix, |(_, e)| e))
}

/// What COPY CODE puts on the clipboard: the code alone, one line.
pub fn copy_text(layout: &Layout) -> Option<String> {
    layout.to_code().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::layout::bundled;

    fn code() -> (Layout, String) {
        let l = bundled::layouts().remove(1);
        let c = l.to_code().unwrap();
        (l, c)
    }

    #[test]
    fn a_bare_code_and_a_layout_file_paste() {
        let (l, c) = code();
        assert_eq!(paste(&c).unwrap().layout, l);
        let file = format!("# my course\n{c}\norigin fork:0x0000000000000005:2:1:e1\n");
        let p = paste(&file).unwrap();
        assert_eq!(p.layout, l);
        assert_eq!(p.origin.as_deref(), Some("fork:0x0000000000000005:2:1:e1"));
        assert_eq!(copy_text(&l).as_deref(), Some(&*c));
    }

    #[test]
    fn wrapped_indented_quoted_and_lower_cased_codes_paste() {
        let (l, c) = code();
        // Wrapped at 76 like a mail, quoted, indented, with CRLF.
        let wrapped: Vec<&str> = c
            .as_bytes()
            .chunks(76)
            .map(|b| std::str::from_utf8(b).unwrap())
            .collect();
        let mail = format!(
            "Try this one:\r\n> {}\r\n\r\nKevin\r\n",
            wrapped.join("\r\n> ")
        );
        assert_eq!(paste(&mail).unwrap().layout, l);
        let spaced = format!(
            "  {}  ",
            c.as_bytes()
                .chunks(8)
                .map(|b| std::str::from_utf8(b).unwrap().to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(" - ")
        );
        assert_eq!(paste(&spaced).unwrap().layout, l);
        let indented = format!("\t{}\n\t{}\n", &c[..300], &c[300..]);
        assert_eq!(paste(&indented).unwrap().layout, l);
    }

    #[test]
    fn words_around_a_code_and_decoys_before_it_do_not_stop_it() {
        let (l, c) = code();
        let chat = format!("This one is HARD, rd: honestly RLY good: {c} - enjoy! RD");
        assert_eq!(paste(&chat).unwrap().layout, l);
        // A blank line ends a code: what follows is not part of it.
        let two = format!("{c}\n\nRD then something");
        assert_eq!(paste(&two).unwrap().layout, l);
    }

    #[test]
    fn text_without_a_code_says_why() {
        let (_, c) = code();
        assert_eq!(paste("hello"), Err(CodeError::Prefix));
        assert_eq!(paste(""), Err(CodeError::Prefix));
        let cut = &c[..c.len() - 10];
        assert!(matches!(
            paste(cut),
            Err(CodeError::Checksum | CodeError::Padding | CodeError::Malformed)
        ));
        let mut typo = c.clone().into_bytes();
        typo[100] = if typo[100] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            paste(std::str::from_utf8(&typo).unwrap()),
            Err(CodeError::Checksum)
        );
        assert_eq!(
            paste(&"x".repeat(4 * MAX_CODE_CHARS + 1)),
            Err(CodeError::TooLong)
        );
    }
}
