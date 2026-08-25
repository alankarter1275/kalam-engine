//! `@font-face` extraction and registration.
//!
//! Stylo parses `@font-face` internally but keeps the rules behind private
//! stylist storage, so — like the fragmentation sidecar — the declarations
//! are pulled out of the same CSS text with cssparser. Registration loads
//! the face bytes into cosmic-text's fontdb and, when the CSS family name
//! differs from the font's internal family name (the common case in the
//! wild), pushes an aliased `FaceInfo` so `font-family: "X"` matches.

use cosmic_text::{fontdb, FontSystem};
use cssparser::{AtRuleParser, ParserState, QualifiedRuleParser, RuleBodyParser, Token};

/// One `@font-face` rule: the CSS family it declares and its source urls
/// (raw, unresolved — resolve against `base`, the declaring stylesheet's
/// container path).
#[derive(Debug, Clone)]
pub struct FontFace {
    pub family: String,
    /// Source urls in declaration order; try until one loads.
    pub sources: Vec<String>,
    /// Container-root path of the stylesheet that declared the rule.
    pub base: String,
}

/// Extract every `@font-face` from `(css text, stylesheet container path)`
/// pairs.
pub fn extract_font_faces(sources: &[(String, String)]) -> Vec<FontFace> {
    let mut faces = Vec::new();
    for (css, base) in sources {
        let mut input = cssparser::ParserInput::new(css);
        let mut parser = cssparser::Parser::new(&mut input);
        let mut rule_parser = FontFaceSheetParser {
            base,
            faces: &mut faces,
        };
        for _ in cssparser::StyleSheetParser::new(&mut parser, &mut rule_parser) {}
    }
    faces
}

/// Load font bytes under a CSS family name. Returns `false` when fontdb
/// rejects the data (corrupt or still-obfuscated font).
pub fn register_font(fonts: &mut FontSystem, css_family: &str, data: Vec<u8>) -> bool {
    let db = fonts.db_mut();
    let before: std::collections::HashSet<fontdb::ID> = db.faces().map(|f| f.id).collect();
    db.load_font_data(data);
    let new_faces: Vec<fontdb::FaceInfo> = db
        .faces()
        .filter(|f| !before.contains(&f.id))
        .cloned()
        .collect();
    if new_faces.is_empty() {
        return false;
    }
    for info in new_faces {
        let has_name = info
            .families
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(css_family));
        if !has_name {
            let mut alias = info;
            alias.families = vec![(
                css_family.to_string(),
                fontdb::Language::English_UnitedStates,
            )];
            db.push_face_info(alias);
        }
    }
    true
}

struct FontFaceSheetParser<'a> {
    base: &'a str,
    faces: &'a mut Vec<FontFace>,
}

impl<'i> AtRuleParser<'i> for FontFaceSheetParser<'_> {
    type Prelude = bool; // is this @font-face?
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::Prelude, cssparser::ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(name.eq_ignore_ascii_case("font-face"))
    }

    fn parse_block<'t>(
        &mut self,
        is_font_face: Self::Prelude,
        _start: &ParserState,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::AtRule, cssparser::ParseError<'i, ()>> {
        if !is_font_face {
            while input.next().is_ok() {}
            return Ok(());
        }
        let mut face = FontFaceDecls::default();
        let mut body = FontFaceDeclParser { face: &mut face };
        for _ in RuleBodyParser::new(input, &mut body) {}
        if let Some(family) = face.family {
            if !face.sources.is_empty() {
                self.faces.push(FontFace {
                    family,
                    sources: face.sources,
                    base: self.base.to_string(),
                });
            }
        }
        Ok(())
    }
}

impl<'i> QualifiedRuleParser<'i> for FontFaceSheetParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::Prelude, cssparser::ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(())
    }

    fn parse_block<'t>(
        &mut self,
        _prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, cssparser::ParseError<'i, ()>> {
        while input.next().is_ok() {}
        Ok(())
    }
}

#[derive(Default)]
struct FontFaceDecls {
    family: Option<String>,
    sources: Vec<String>,
}

struct FontFaceDeclParser<'a> {
    face: &'a mut FontFaceDecls,
}

impl<'i> cssparser::DeclarationParser<'i> for FontFaceDeclParser<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut cssparser::Parser<'i, 't>,
        _state: &ParserState,
    ) -> Result<(), cssparser::ParseError<'i, ()>> {
        if name.eq_ignore_ascii_case("font-family") {
            match input.next() {
                Ok(Token::QuotedString(s)) => self.face.family = Some(s.to_string()),
                Ok(Token::Ident(s)) => {
                    // Unquoted family names may span several idents.
                    let mut family = s.to_string();
                    while let Ok(Token::Ident(next)) = input.next() {
                        family.push(' ');
                        family.push_str(next);
                    }
                    self.face.family = Some(family);
                }
                _ => {}
            }
        } else if name.eq_ignore_ascii_case("src") {
            // Collect url() sources; local(...) and format(...) are skipped.
            loop {
                match input.next() {
                    Ok(Token::UnquotedUrl(url)) => self.face.sources.push(url.to_string()),
                    Ok(Token::Function(f)) if f.eq_ignore_ascii_case("url") => {
                        let url = input.parse_nested_block(
                            |i| -> Result<String, cssparser::ParseError<'i, ()>> {
                                match i.next() {
                                    Ok(Token::QuotedString(s)) => Ok(s.to_string()),
                                    Ok(Token::UnquotedUrl(s)) => Ok(s.to_string()),
                                    _ => Err(i.new_custom_error(())),
                                }
                            },
                        );
                        if let Ok(url) = url {
                            self.face.sources.push(url);
                        }
                    }
                    Ok(Token::Function(_)) => {
                        let _ = input.parse_nested_block(
                            |i| -> Result<(), cssparser::ParseError<'i, ()>> {
                                while i.next().is_ok() {}
                                Ok(())
                            },
                        );
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        while input.next().is_ok() {}
        Ok(())
    }
}

impl<'i> cssparser::AtRuleParser<'i> for FontFaceDeclParser<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> cssparser::QualifiedRuleParser<'i> for FontFaceDeclParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> cssparser::RuleBodyItemParser<'i, (), ()> for FontFaceDeclParser<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_family_and_sources() {
        let css = r#"
            body { color: red; }
            @font-face {
                font-family: "Fixture Serif";
                src: local("Nope"), url(../fonts/a.ttf) format("truetype"), url("b.otf");
            }
            @font-face { font-family: Bare Name Font; src: url(c.woff); }
            @media print { .x { color: blue; } }
        "#;
        let faces = extract_font_faces(&[(css.to_string(), "OEBPS/css/style.css".to_string())]);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].family, "Fixture Serif");
        assert_eq!(faces[0].sources, vec!["../fonts/a.ttf", "b.otf"]);
        assert_eq!(faces[0].base, "OEBPS/css/style.css");
        assert_eq!(faces[1].family, "Bare Name Font");
        assert_eq!(faces[1].sources, vec!["c.woff"]);
    }

    #[test]
    fn registers_under_alias() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/fonts/CrimsonText-Bold.ttf");
        let data = std::fs::read(dir).unwrap();
        let source = chapbook_core::FontSource::embedded(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
            "Crimson Text",
        );
        let (mut fonts, _) = crate::build_font_system(&source).unwrap();
        assert!(register_font(&mut fonts, "Totally Custom Family", data));
        let found = fonts
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n == "Totally Custom Family"));
        assert!(found, "alias family must be queryable");
    }

    #[test]
    fn rejects_garbage_font() {
        let mut fonts = cosmic_text::FontSystem::new_with_locale_and_db(
            "en-US".into(),
            cosmic_text::fontdb::Database::new(),
        );
        assert!(!register_font(&mut fonts, "X", vec![0u8; 64]));
    }
}
