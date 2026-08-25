//! EPUB Canonical Fragment Identifiers (the syntax layer).
//!
//! CFIs are the industry interchange format for reading positions:
//! `epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10)`. This module
//! parses and serializes the *string* form into a structured [`Cfi`];
//! turning one into or out of a chapbook locator offset needs the DOM and
//! lives in `chapbook_layout::dom::cfi`.
//!
//! Persistence stays `LayeredLocator` (richer re-anchoring, see
//! `docs/LOCATORS.md`) — CFI is import/export interop.
//!
//! Supported subset (the book-position shape): one package part, one `!`
//! indirection, element/character-data steps, an optional terminating
//! character offset. Parsed but discarded: ID assertions are kept (they
//! drive correction during resolution), text-location assertions
//! (`:10[pre,post]`) and side bias are dropped, spatial (`@x:y`) and
//! temporal (`~t`) terminus are rejected as unsupported, as are range CFIs
//! (`base,start,end`) and multiple indirections.

use crate::error::ChapbookError;

/// One CFI path step: `/index[assertion]`. Even indices address element
/// children (2 = first element child); odd indices address the virtual
/// character-data span between them (1 = before the first element child).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfiStep {
    pub index: u32,
    /// `[id]` assertion, unescaped. On element steps this names the target
    /// element's `id` and corrects a stale index during resolution.
    pub assertion: Option<String>,
}

/// A parsed point CFI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cfi {
    /// Steps before the `!`, addressing into the package document
    /// (canonically `/6/K`: the `<spine>`, then the K/2-th `<itemref>`).
    pub package_steps: Vec<CfiStep>,
    /// Steps after the `!`, addressing into the content document.
    pub doc_steps: Vec<CfiStep>,
    /// Terminating `:N` character offset into the character-data span
    /// addressed by the last (odd) step.
    pub char_offset: Option<u32>,
}

impl Cfi {
    /// The spine index addressed by the package part, when it has the
    /// canonical `/6/K` shape (K even). EPUB fixes the `<package>` child
    /// order (metadata, manifest, spine), so `/6` is the spine and the
    /// itemref index maps directly.
    pub fn spine_index(&self) -> Option<usize> {
        let last = self.package_steps.last()?;
        if self.package_steps.len() < 2 || last.index % 2 != 0 || last.index == 0 {
            return None;
        }
        Some((last.index / 2 - 1) as usize)
    }

    /// The canonical package part for a spine index: `/6/K`.
    pub fn package_steps_for_spine(spine_index: usize) -> Vec<CfiStep> {
        vec![
            CfiStep {
                index: 6,
                assertion: None,
            },
            CfiStep {
                index: 2 * (spine_index as u32 + 1),
                assertion: None,
            },
        ]
    }

    /// Parse a CFI, with or without the `epubcfi(...)` wrapper.
    pub fn parse(input: &str) -> Result<Cfi, ChapbookError> {
        let err = |m: &str| ChapbookError::Cfi(format!("{m} in {input:?}"));
        let s = input.trim();
        let s = match s.strip_prefix("epubcfi(") {
            Some(rest) => rest
                .strip_suffix(')')
                .ok_or_else(|| err("missing closing parenthesis"))?,
            None => s,
        };
        if s.is_empty() {
            return Err(err("empty CFI"));
        }

        let mut package_steps = Vec::new();
        let mut doc_steps = Vec::new();
        let mut char_offset = None;
        let mut in_doc = false;

        let mut chars = s.chars().peekable();
        loop {
            match chars.peek().copied() {
                None => break,
                Some('!') => {
                    if in_doc {
                        return Err(err("multiple indirections are unsupported"));
                    }
                    in_doc = true;
                    chars.next();
                }
                Some('/') => {
                    chars.next();
                    let index = parse_int(&mut chars).ok_or_else(|| err("bad step index"))?;
                    let assertion = parse_assertion(&mut chars)?;
                    let step = CfiStep { index, assertion };
                    if in_doc {
                        doc_steps.push(step);
                    } else {
                        package_steps.push(step);
                    }
                }
                Some(':') => {
                    chars.next();
                    char_offset =
                        Some(parse_int(&mut chars).ok_or_else(|| err("bad character offset"))?);
                    // A text-location assertion after the offset is
                    // parsed and dropped.
                    parse_assertion(&mut chars)?;
                    if chars.peek().is_some() {
                        return Err(err("trailing content after character offset"));
                    }
                    break;
                }
                Some(',') => return Err(err("range CFIs are unsupported")),
                Some('@') | Some('~') => {
                    return Err(err("spatial/temporal terminus is unsupported"))
                }
                Some(other) => {
                    return Err(ChapbookError::Cfi(format!(
                        "unexpected {other:?} in {input:?}"
                    )))
                }
            }
        }

        if !in_doc {
            return Err(err("missing '!' indirection"));
        }
        if doc_steps.is_empty() {
            return Err(err("no content-document steps"));
        }
        Ok(Cfi {
            package_steps,
            doc_steps,
            char_offset,
        })
    }
}

impl std::fmt::Display for Cfi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "epubcfi(")?;
        for step in &self.package_steps {
            write_step(f, step)?;
        }
        write!(f, "!")?;
        for step in &self.doc_steps {
            write_step(f, step)?;
        }
        if let Some(offset) = self.char_offset {
            write!(f, ":{offset}")?;
        }
        write!(f, ")")
    }
}

fn write_step(f: &mut std::fmt::Formatter<'_>, step: &CfiStep) -> std::fmt::Result {
    write!(f, "/{}", step.index)?;
    if let Some(assertion) = &step.assertion {
        write!(f, "[")?;
        for c in assertion.chars() {
            if matches!(c, '^' | '[' | ']' | '(' | ')' | ',' | ';') {
                write!(f, "^")?;
            }
            write!(f, "{c}")?;
        }
        write!(f, "]")?;
    }
    Ok(())
}

fn parse_int(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<u32> {
    let mut value: u32 = 0;
    let mut any = false;
    while let Some(c) = chars.peek() {
        let Some(d) = c.to_digit(10) else { break };
        value = value.checked_mul(10)?.checked_add(d)?;
        any = true;
        chars.next();
    }
    any.then_some(value)
}

/// Parse an optional `[...]` assertion, handling `^` escapes. Returns the
/// unescaped content; assertions containing parameters or a comma (text
/// location assertions, side bias) are parsed but yield `None`.
fn parse_assertion(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<Option<String>, ChapbookError> {
    if chars.peek() != Some(&'[') {
        return Ok(None);
    }
    chars.next();
    let mut content = String::new();
    let mut plain_id = true;
    loop {
        match chars.next() {
            None => return Err(ChapbookError::Cfi("unterminated assertion".into())),
            Some(']') => break,
            Some('^') => match chars.next() {
                Some(escaped) => content.push(escaped),
                None => return Err(ChapbookError::Cfi("dangling ^ escape".into())),
            },
            Some(c @ (',' | ';')) => {
                // Text-location assertion or parameter: not an ID.
                plain_id = false;
                content.push(c);
            }
            Some(c) => content.push(c),
        }
    }
    Ok((plain_id && !content.is_empty()).then_some(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        let cfi = Cfi::parse("epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10)").unwrap();
        assert_eq!(cfi.spine_index(), Some(1));
        assert_eq!(cfi.package_steps[1].assertion.as_deref(), Some("chap01ref"));
        assert_eq!(cfi.doc_steps.len(), 3);
        assert_eq!(cfi.doc_steps[0].index, 4);
        assert_eq!(cfi.doc_steps[0].assertion.as_deref(), Some("body01"));
        assert_eq!(cfi.doc_steps[2].index, 3);
        assert_eq!(cfi.char_offset, Some(10));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10)",
            "epubcfi(/6/2!/4/2/1:0)",
            "epubcfi(/6/14!/4/6[with^]bracket]/5:2)",
        ] {
            let cfi = Cfi::parse(s).unwrap();
            assert_eq!(cfi.to_string(), s);
            assert_eq!(Cfi::parse(&cfi.to_string()).unwrap(), cfi);
        }
    }

    #[test]
    fn text_location_assertion_is_tolerated_and_dropped() {
        let cfi = Cfi::parse("epubcfi(/6/4!/4/10/3:10[don,key])").unwrap();
        assert_eq!(cfi.char_offset, Some(10));
    }

    #[test]
    fn unsupported_shapes_are_rejected() {
        for s in [
            "epubcfi(/6/4!/4/2,/2/1:0,/4/1:5)", // range
            "epubcfi(/6/4!/4/2!/2/1:0)",        // double indirection
            "epubcfi(/6/4!/4/2@3:7)",           // spatial
            "epubcfi(/6/4)",                    // no indirection
            "epubcfi(/6/4!/4/2:1",              // unclosed wrapper
        ] {
            assert!(Cfi::parse(s).is_err(), "{s} should be rejected");
        }
    }

    #[test]
    fn spine_round_trip() {
        for spine in [0usize, 1, 7, 40] {
            let steps = Cfi::package_steps_for_spine(spine);
            let cfi = Cfi {
                package_steps: steps,
                doc_steps: vec![CfiStep {
                    index: 4,
                    assertion: None,
                }],
                char_offset: None,
            };
            assert_eq!(cfi.spine_index(), Some(spine));
        }
    }
}
