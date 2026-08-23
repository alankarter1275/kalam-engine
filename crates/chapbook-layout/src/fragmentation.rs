//! The fragmentation sidecar cascade.
//!
//! Servo-mode stylo does not implement the CSS fragmentation properties
//! (`break-before/after/inside`, the legacy `page-break-*` aliases,
//! `widows`, `orphans` — all Gecko-only), so pagination gets them from this
//! mini-cascade: cssparser pulls just these declarations out of the same
//! stylesheets, and their selectors match through chapbook-dom's
//! `selectors::Element` impl with standard specificity/source-order rules.
//! `hyphens` (also Gecko-only, inherited) rides along for the same reason.
//!
//! `@media` blocks are descended into when their query can apply on screen
//! (`screen`, `all`, or bare feature queries); `print`-only blocks are
//! skipped, matching the style engine's screen device. Other at-rules'
//! contents are ignored.

use std::collections::HashMap;

use cssparser::{AtRuleParser, ParserState, QualifiedRuleParser, RuleBodyParser};
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use selectors::matching::matches_selector;
use selectors::parser::Selector;
use style::selector_parser::{SelectorImpl, SelectorParser};
use style::stylesheets::UrlExtraData;

use chapbook_dom::{Document, NodeData, NodeId};

/// Forced/avoided break on a box edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BreakRule {
    #[default]
    Auto,
    /// `page` / `always` / `left` / `right` — force a page break.
    Page,
    /// `avoid` / `avoid-page` — avoid a break here if possible.
    Avoid,
}

/// The resolved fragmentation style of one element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragStyle {
    pub break_before: BreakRule,
    pub break_after: BreakRule,
    pub break_inside_avoid: bool,
    pub widows: u32,
    pub orphans: u32,
    /// `hyphens: auto` — also Gecko-only in servo-mode stylo, and inherited,
    /// so it rides in this sidecar next to widows/orphans.
    pub hyphens_auto: bool,
}

impl Default for FragStyle {
    fn default() -> Self {
        FragStyle {
            break_before: BreakRule::Auto,
            break_after: BreakRule::Auto,
            break_inside_avoid: false,
            widows: 2,
            orphans: 2,
            hyphens_auto: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FragDecls {
    break_before: Option<BreakRule>,
    break_after: Option<BreakRule>,
    break_inside_avoid: Option<bool>,
    widows: Option<u32>,
    orphans: Option<u32>,
    hyphens_auto: Option<bool>,
}

impl FragDecls {
    fn is_empty(&self) -> bool {
        self.break_before.is_none()
            && self.break_after.is_none()
            && self.break_inside_avoid.is_none()
            && self.widows.is_none()
            && self.orphans.is_none()
            && self.hyphens_auto.is_none()
    }
}

struct FragRule {
    selector: Selector<SelectorImpl>,
    specificity: u32,
    order: usize,
    decls: FragDecls,
}

/// All fragmentation rules of a chapter's stylesheets, ready to match.
pub struct FragRules {
    rules: Vec<FragRule>,
}

impl FragRules {
    /// Extract fragmentation declarations from stylesheets, in cascade order
    /// (later sheet = higher order). Widows/orphans are inherited properties,
    /// so resolution walks ancestors when unset.
    pub fn parse(css_sources: &[String]) -> Self {
        let url_data = UrlExtraData(style::servo_arc::Arc::new(
            url::Url::parse("chapbook:///frag.css").unwrap(),
        ));
        let mut rules = Vec::new();
        let mut order = 0usize;
        for css in css_sources {
            collect_rules(css, &url_data, &mut rules, &mut order);
        }
        FragRules { rules }
    }

    /// Resolve every element's [`FragStyle`] in one pass. Widows/orphans
    /// inherit; break-* do not.
    pub fn resolve(&self, doc: &Document) -> HashMap<NodeId, FragStyle> {
        let mut caches = SelectorCaches::default();
        let mut out: HashMap<NodeId, FragStyle> = HashMap::new();
        let Some(html) = doc.document_element() else {
            return out;
        };
        self.resolve_into(doc, html, FragStyle::default(), &mut caches, &mut out);
        out
    }

    fn resolve_into(
        &self,
        doc: &Document,
        id: NodeId,
        inherited: FragStyle,
        caches: &mut SelectorCaches,
        out: &mut HashMap<NodeId, FragStyle>,
    ) {
        if !matches!(doc.node(id).data, NodeData::Element(_)) {
            return;
        }

        // Non-inherited properties reset; widows/orphans/hyphens carry down.
        let mut style = FragStyle {
            widows: inherited.widows,
            orphans: inherited.orphans,
            hyphens_auto: inherited.hyphens_auto,
            ..FragStyle::default()
        };

        // Matching candidates in ascending (specificity, order): later
        // applications overwrite earlier ones per property.
        let element = doc.handle(id);
        let mut matched: Vec<&FragRule> = self
            .rules
            .iter()
            .filter(|rule| {
                let mut context = MatchingContext::new(
                    MatchingMode::Normal,
                    None,
                    caches,
                    QuirksMode::NoQuirks,
                    NeedsSelectorFlags::No,
                    MatchingForInvalidation::No,
                );
                matches_selector(&rule.selector, 0, None, &element, &mut context)
            })
            .collect();
        matched.sort_by_key(|rule| (rule.specificity, rule.order));
        for rule in matched {
            let d = &rule.decls;
            if let Some(v) = d.break_before {
                style.break_before = v;
            }
            if let Some(v) = d.break_after {
                style.break_after = v;
            }
            if let Some(v) = d.break_inside_avoid {
                style.break_inside_avoid = v;
            }
            if let Some(v) = d.widows {
                style.widows = v;
            }
            if let Some(v) = d.orphans {
                style.orphans = v;
            }
            if let Some(v) = d.hyphens_auto {
                style.hyphens_auto = v;
            }
        }

        out.insert(id, style);
        for child in &doc.node(id).children {
            self.resolve_into(doc, *child, style, caches, out);
        }
    }
}

fn collect_rules(css: &str, url_data: &UrlExtraData, rules: &mut Vec<FragRule>, order: &mut usize) {
    let mut input = cssparser::ParserInput::new(css);
    let mut parser = cssparser::Parser::new(&mut input);
    let mut rule_parser = SheetParser {
        url_data,
        rules,
        order,
    };
    for _ in cssparser::StyleSheetParser::new(&mut parser, &mut rule_parser) {
        // Rules are pushed by the parser callbacks; per-rule errors are
        // recovered by the driver.
    }
}

/// Whether an `@media` prelude can apply on the screen device: `screen`,
/// `all`, and bare feature queries pass; `print`-only blocks are skipped.
fn media_query_applies(prelude: &str) -> bool {
    let p = prelude.to_ascii_lowercase();
    let mentions_screen = p.contains("screen") || p.contains("all");
    let mentions_print = p.contains("print");
    !mentions_print || mentions_screen
}

struct SheetParser<'a> {
    url_data: &'a UrlExtraData,
    rules: &'a mut Vec<FragRule>,
    order: &'a mut usize,
}

impl<'i> QualifiedRuleParser<'i> for SheetParser<'_> {
    type Prelude = Option<selectors::SelectorList<SelectorImpl>>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::Prelude, cssparser::ParseError<'i, ()>> {
        // Capture the raw prelude text and hand it to stylo's selector
        // parser, which speaks the same SelectorImpl our Element impl does.
        let start = input.position();
        while input.next().is_ok() {}
        let text = input.slice_from(start);
        Ok(SelectorParser::parse_author_origin_no_namespace(text, self.url_data).ok())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, cssparser::ParseError<'i, ()>> {
        let mut decls = FragDecls::default();
        let mut body = DeclParser { decls: &mut decls };
        for _ in RuleBodyParser::new(input, &mut body) {
            // Errors on individual declarations are ignored; we only collect
            // the fragmentation subset and skip everything else.
        }
        if let Some(selectors) = prelude {
            if !decls.is_empty() {
                *self.order += 1;
                for selector in selectors.slice() {
                    self.rules.push(FragRule {
                        selector: selector.clone(),
                        specificity: selector.specificity(),
                        order: *self.order,
                        decls,
                    });
                }
            }
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for SheetParser<'_> {
    /// `Some(true)` = an applicable `@media` block to descend into.
    type Prelude = bool;
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::Prelude, cssparser::ParseError<'i, ()>> {
        let start = input.position();
        while input.next().is_ok() {}
        let prelude = input.slice_from(start);
        Ok(name.eq_ignore_ascii_case("media") && media_query_applies(prelude))
    }

    fn parse_block<'t>(
        &mut self,
        descend: Self::Prelude,
        _start: &ParserState,
        input: &mut cssparser::Parser<'i, 't>,
    ) -> Result<Self::AtRule, cssparser::ParseError<'i, ()>> {
        let start = input.position();
        while input.next().is_ok() {}
        if descend {
            let inner = input.slice_from(start).to_string();
            collect_rules(&inner, self.url_data, self.rules, self.order);
        }
        Ok(())
    }
}

struct DeclParser<'a> {
    decls: &'a mut FragDecls,
}

impl<'i> cssparser::DeclarationParser<'i> for DeclParser<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut cssparser::Parser<'i, 't>,
        _state: &ParserState,
    ) -> Result<(), cssparser::ParseError<'i, ()>> {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "break-before" | "page-break-before" => {
                self.decls.break_before = parse_break_value(input);
            }
            "break-after" | "page-break-after" => {
                self.decls.break_after = parse_break_value(input);
            }
            "break-inside" | "page-break-inside" => {
                let ident = input.expect_ident().map(|i| i.to_ascii_lowercase());
                self.decls.break_inside_avoid = match ident.as_deref() {
                    Ok("avoid") | Ok("avoid-page") => Some(true),
                    Ok("auto") => Some(false),
                    _ => None,
                };
            }
            "widows" => {
                if let Ok(n) = input.expect_integer() {
                    if n >= 1 {
                        self.decls.widows = Some(n as u32);
                    }
                }
            }
            "orphans" => {
                if let Ok(n) = input.expect_integer() {
                    if n >= 1 {
                        self.decls.orphans = Some(n as u32);
                    }
                }
            }
            // EPUB CSS commonly carries the vendor-prefixed spellings.
            "hyphens" | "-epub-hyphens" | "-webkit-hyphens" | "-moz-hyphens" => {
                let ident = input.expect_ident().map(|i| i.to_ascii_lowercase());
                self.decls.hyphens_auto = match ident.as_deref() {
                    Ok("auto") => Some(true),
                    Ok("manual") | Ok("none") => Some(false),
                    _ => None,
                };
            }
            _ => {}
        }
        // Consume any remainder so the body parser can continue cleanly.
        while input.next().is_ok() {}
        Ok(())
    }
}

impl<'i> cssparser::AtRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> cssparser::QualifiedRuleParser<'i> for DeclParser<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> cssparser::RuleBodyItemParser<'i, (), ()> for DeclParser<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

fn parse_break_value(input: &mut cssparser::Parser<'_, '_>) -> Option<BreakRule> {
    let ident = input.expect_ident().map(|i| i.to_ascii_lowercase()).ok()?;
    match ident.as_str() {
        "page" | "always" | "left" | "right" | "recto" | "verso" => Some(BreakRule::Page),
        "avoid" | "avoid-page" => Some(BreakRule::Avoid),
        "auto" => Some(BreakRule::Auto),
        _ => None,
    }
}
