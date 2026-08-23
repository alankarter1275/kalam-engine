//! The stylo DOM-trait bindings: how this arena DOM participates in servo's
//! style system.
//!
//! Structure ported from blitz-dom's proven `stylo.rs` (same stylo 0.20
//! lockstep), simplified for chapbook's model: documents are static after
//! parse, so there are no snapshots, no incremental restyle, no animations,
//! no shadow DOM, and no interactive element states. The style traversal runs
//! sequentially.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use markup5ever::{local_name, LocalName, LocalNameStaticSet, Namespace, NamespaceStaticSet};
use selectors::bloom::BLOOM_HASH_MASK;
use selectors::{
    attr::{AttrSelectorOperation, NamespaceConstraint},
    matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode},
    sink::Push,
    OpaqueElement,
};
use slotmap::Key;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::bloom::each_relevant_element_hash;
use style::context::{QuirksMode, SharedStyleContext};
use style::data::{ElementDataMut, ElementDataRef};
use style::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use style::properties::{
    ComputedValues, Importance, PropertyDeclaration, PropertyDeclarationBlock,
};
use style::rule_tree::{CascadeLevel, CascadeOrigin};
use style::selector_parser::{NonTSPseudoClass, PseudoElement, RestyleDamage, SelectorImpl};
use style::servo_arc::{Arc as ServoArc, ArcBorrow};
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::layer_rule::LayerOrder;
use style::stylesheets::scope_rule::ImplicitScopeRoot;
use style::values::{AtomIdent, AtomString, GenericAtomIdent};
use style::{Atom, CaseSensitivityExt};
use style_dom::ElementState;

use crate::tree::{DocumentInner, ElementData, Node, NodeData, NodeId};

/// A copyable, **pointer-sized** handle to one node of a [`Document`] — the
/// type stylo's DOM traits are implemented on.
///
/// Must stay pointer-sized: stylo's style sharing cache statically asserts
/// `size_of::<StyleSharingCandidate<E>>` against a cache built for a
/// one-word element handle. Nodes carry a back-pointer to their document
/// (sealed after parse), which is what lets a bare `&Node` navigate the tree.
#[derive(Clone, Copy)]
pub struct DomNode<'a>(&'a Node);

impl<'a> DomNode<'a> {
    pub fn id(&self) -> NodeId {
        self.0.id
    }

    fn doc(&self) -> &'a DocumentInner {
        // SAFETY-adjacent: `document()` deref of the sealed back-pointer; the
        // returned borrow is tied to 'a because the node borrows from the
        // same boxed DocumentInner.
        unsafe { &*(self.0.document() as *const DocumentInner) }
    }

    fn node(&self) -> &'a Node {
        self.0
    }

    fn with(&self, id: NodeId) -> Self {
        DomNode(self.doc().node(id))
    }

    fn element(&self) -> Option<&'a ElementData> {
        match &self.node().data {
            NodeData::Element(el) => Some(el),
            _ => None,
        }
    }

    fn expect_element(&self) -> &'a ElementData {
        self.element().expect("not an element")
    }

    fn ffi_id(&self) -> u64 {
        self.0.id.data().as_ffi()
    }
}

const _: () = assert!(
    std::mem::size_of::<DomNode<'_>>() == std::mem::size_of::<usize>(),
    "DomNode must stay pointer-sized for stylo's style sharing cache"
);

impl DocumentInner {
    /// Handle to the document root, for driving the style traversal.
    pub fn style_root(&self) -> DomNode<'_> {
        DomNode(self.node(self.root()))
    }

    /// Handle to the `<html>` element.
    pub fn style_root_element(&self) -> Option<DomNode<'_>> {
        self.document_element().map(|id| DomNode(self.node(id)))
    }

    /// The primary computed style landed on `id` by the last style traversal.
    pub fn primary_styles(&self, id: NodeId) -> Option<ServoArc<ComputedValues>> {
        self.node(id)
            .stylo
            .data
            .get()
            .and_then(|data| data.styles.get_primary().cloned())
    }
}

impl PartialEq for DomNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for DomNode<'_> {}

impl Hash for DomNode<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.ffi_id());
    }
}

impl fmt::Debug for DomNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.node().data {
            NodeData::Element(el) => write!(f, "DomNode(<{}>)", el.name.local),
            NodeData::Text(_) => write!(f, "DomNode(#text)"),
            NodeData::Comment => write!(f, "DomNode(#comment)"),
            NodeData::Document => write!(f, "DomNode(#document)"),
        }
    }
}

impl<'a> TDocument for DomNode<'a> {
    type ConcreteNode = DomNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn is_html_document(&self) -> bool {
        // XHTML content parses through the HTML algorithm with lowercased
        // names; HTML-document matching semantics are what we want.
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        self.doc().shared_lock()
    }
}

impl NodeInfo for DomNode<'_> {
    fn is_element(&self) -> bool {
        matches!(self.node().data, NodeData::Element(_))
    }

    fn is_text_node(&self) -> bool {
        matches!(self.node().data, NodeData::Text(_))
    }
}

impl<'a> TShadowRoot for DomNode<'a> {
    type ConcreteNode = DomNode<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        unreachable!("no shadow DOM: TNode::as_shadow_root always returns None")
    }

    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        unreachable!("no shadow DOM: TNode::as_shadow_root always returns None")
    }
}

impl<'a> TNode for DomNode<'a> {
    type ConcreteElement = DomNode<'a>;
    type ConcreteDocument = DomNode<'a>;
    type ConcreteShadowRoot = DomNode<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.node().parent.map(|id| self.with(id))
    }

    fn first_child(&self) -> Option<Self> {
        self.node().children.first().map(|id| self.with(*id))
    }

    fn last_child(&self) -> Option<Self> {
        self.node().children.last().map(|id| self.with(*id))
    }

    fn prev_sibling(&self) -> Option<Self> {
        self.doc().sibling(self.id(), -1).map(|id| self.with(id))
    }

    fn next_sibling(&self) -> Option<Self> {
        self.doc().sibling(self.id(), 1).map(|id| self.with(id))
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.with(self.doc().root())
    }

    fn is_in_document(&self) -> bool {
        true
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node().and_then(|n| n.as_element())
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.ffi_id() as usize)
    }

    fn debug_id(self) -> usize {
        self.ffi_id() as usize
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        match self.node().data {
            NodeData::Element(_) => Some(*self),
            _ => None,
        }
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        match self.node().data {
            NodeData::Document => Some(*self),
            _ => None,
        }
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }
}

impl selectors::Element for DomNode<'_> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        let non_null = NonNull::new((self.ffi_id() as usize).wrapping_add(1) as *mut ()).unwrap();
        OpaqueElement::from_non_null_ptr(non_null)
    }

    fn parent_element(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut n = -1isize;
        while let Some(id) = self.doc().sibling(self.id(), n) {
            let node = self.with(id);
            if node.is_element() {
                return Some(node);
            }
            n -= 1;
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut n = 1isize;
        while let Some(id) = self.doc().sibling(self.id(), n) {
            let node = self.with(id);
            if node.is_element() {
                return Some(node);
            }
            n += 1;
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        self.node()
            .children
            .iter()
            .map(|id| self.with(*id))
            .find(|child| child.is_element())
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(&self, local_name: &LocalName) -> bool {
        self.element()
            .is_some_and(|el| el.name.local == *local_name)
    }

    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.expect_element().name.ns == *ns
    }

    fn is_same_type(&self, other: &Self) -> bool {
        let (a, b) = (self.expect_element(), other.expect_element());
        a.name.local == b.name.local && a.name.ns == b.name.ns
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&GenericAtomIdent<NamespaceStaticSet>>,
        local_name: &GenericAtomIdent<LocalNameStaticSet>,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        match self.element().and_then(|el| el.attr(&local_name.0)) {
            Some(value) => operation.eval_str(value),
            None => false,
        }
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo_class: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        // No interactivity, no forms, no visited history: only the static
        // link pseudo-classes can match.
        match *pseudo_class {
            NonTSPseudoClass::Link | NonTSPseudoClass::AnyLink => self.is_link(),
            _ => false,
        }
    }

    fn match_pseudo_element(
        &self,
        pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        self.node()
            .stylo
            .data
            .get()
            .and_then(|data| data.styles.get_primary().and_then(|s| s.pseudo()))
            .is_some_and(|pseudo| pseudo == *pe)
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            let cell = &self.node().stylo.selector_flags;
            cell.set(cell.get() | self_flags);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty() {
            if let Some(parent) = self.parent_node() {
                let cell = &parent.node().stylo.selector_flags;
                cell.set(cell.get() | parent_flags);
            }
        }
    }

    fn is_link(&self) -> bool {
        self.element().is_some_and(|el| {
            (el.name.local == local_name!("a") || el.name.local == local_name!("area"))
                && el.attr(&local_name!("href")).is_some()
        })
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(
        &self,
        id: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        self.element()
            .and_then(|el| el.id.as_ref())
            .is_some_and(|id_attr| case_sensitivity.eq_atom(id_attr, id))
    }

    fn has_class(
        &self,
        search_name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
        case_sensitivity: selectors::attr::CaseSensitivity,
    ) -> bool {
        self.element().is_some_and(|el| {
            el.classes.iter().any(|class| {
                let atom = Atom::from(class.as_str());
                case_sensitivity.eq_atom(&atom, search_name)
            })
        })
    }

    fn imported_part(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> Option<<Self::Impl as selectors::SelectorImpl>::Identifier> {
        None
    }

    fn is_part(&self, _name: &<Self::Impl as selectors::SelectorImpl>::Identifier) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.node().children.is_empty()
    }

    fn is_root(&self) -> bool {
        self.parent_node()
            .and_then(|parent| parent.parent_node())
            .is_none()
    }

    fn has_custom_state(
        &self,
        _name: &<Self::Impl as selectors::SelectorImpl>::Identifier,
    ) -> bool {
        false
    }

    fn add_element_unique_hashes(&self, filter: &mut selectors::bloom::BloomFilter) -> bool {
        each_relevant_element_hash(*self, |hash| filter.insert_hash(hash & BLOOM_HASH_MASK));
        true
    }
}

impl<'a> TElement for DomNode<'a> {
    type ConcreteNode = DomNode<'a>;
    type TraversalChildrenIterator = Traverser<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        None
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Traverser {
            parent: *self,
            child_index: 0,
        })
    }

    fn is_html_element(&self) -> bool {
        self.is_element()
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.expect_element()
            .style_attribute
            .as_ref()
            .map(|block| block.borrow_arc())
    }

    fn state(&self) -> ElementState {
        ElementState::empty()
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&Atom> {
        self.element().and_then(|el| el.id.as_ref())
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        if let Some(el) = self.element() {
            for class in &el.classes {
                let atom = Atom::from(class.as_str());
                callback(AtomIdent::cast(&atom));
            }
        }
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        if let Some(el) = self.element() {
            for (name, _) in &el.attrs {
                callback(&GenericAtomIdent(name.local.clone()));
            }
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        self.node().stylo.dirty_descendants.get()
    }

    fn has_snapshot(&self) -> bool {
        false
    }

    fn handled_snapshot(&self) -> bool {
        self.node().stylo.snapshot_handled.load(Ordering::SeqCst)
    }

    unsafe fn set_handled_snapshot(&self) {
        self.node()
            .stylo
            .snapshot_handled
            .store(true, Ordering::SeqCst);
    }

    unsafe fn set_dirty_descendants(&self) {
        self.node().stylo.dirty_descendants.set(true);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.node().stylo.dirty_descendants.set(false);
    }

    fn store_children_to_process(&self, _n: isize) {
        unimplemented!("only needed for postorder traversal, which is disabled")
    }

    fn did_process_child(&self) -> isize {
        unimplemented!("only needed for postorder traversal, which is disabled")
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: stylo's traversal has exclusive access to this node.
        unsafe { self.node().stylo.data.ensure_init() }
    }

    unsafe fn clear_data(&self) {
        // SAFETY: stylo's traversal has exclusive access to this node.
        unsafe { self.node().stylo.data.clear() }
    }

    fn has_data(&self) -> bool {
        self.node().stylo.data.has_data()
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.node().stylo.data.get()
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        unsafe { self.node().stylo.data.unsafe_stylo_only_mut() }
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _context: &SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn has_css_transitions(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }

    fn animation_rule(
        &self,
        _context: &SharedStyleContext,
    ) -> Option<ServoArc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn transition_rule(
        &self,
        _context: &SharedStyleContext,
    ) -> Option<ServoArc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn get_attr(&self, attr: &style::LocalName, _ns: &style::Namespace) -> Option<String> {
        self.element()
            .and_then(|el| el.attr(&attr.0))
            .map(str::to_string)
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        None
    }

    fn match_element_lang(
        &self,
        _override_lang: Option<Option<style::selector_parser::AttrValue>>,
        _value: &style::selector_parser::Lang,
    ) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        let is_body = self
            .element()
            .is_some_and(|el| el.name.local == local_name!("body"));
        is_body && self.node().parent == self.doc().document_element()
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
        let Some(el) = self.element() else {
            return;
        };

        let mut push_style = |decl: PropertyDeclaration| {
            hints.push(ApplicableDeclarationBlock::from_declarations(
                ServoArc::new(
                    self.doc()
                        .shared_lock()
                        .wrap(PropertyDeclarationBlock::with_one(decl, Importance::Normal)),
                ),
                CascadeLevel::new(CascadeOrigin::PresHints),
                LayerOrder::root(),
            ));
        };

        // The subset of legacy presentational attributes that matter for
        // book content: the `hidden` attribute, and width/height on images.
        for (name, value) in &el.attrs {
            if !name.ns.is_empty() {
                continue;
            }
            if name.local == local_name!("hidden") {
                use style::values::specified::Display;
                push_style(PropertyDeclaration::Display(Display::None));
            }

            let is_width = name.local == local_name!("width");
            let is_height = name.local == local_name!("height");
            if (is_width || is_height) && el.name.local == local_name!("img") {
                if let Some(size) = parse_dimension_attr(value) {
                    use style::values::generics::{length::Size, NonNegative};
                    let size = Size::LengthPercentage(NonNegative(size));
                    push_style(if is_width {
                        PropertyDeclaration::Width(size)
                    } else {
                        PropertyDeclaration::Height(size)
                    });
                }
            }
        }
    }

    fn local_name(&self) -> &LocalName {
        &self.expect_element().name.local
    }

    fn namespace(&self) -> &Namespace {
        &self.expect_element().name.ns
    }

    fn query_container_size(
        &self,
        _display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        // Container queries unsupported; this disables them without panicking.
        Default::default()
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.node().stylo.selector_flags.get().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        let flags = self.node().stylo.selector_flags.get();
        if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING)
        {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING
        } else if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR)
        {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR
        } else if flags.contains(ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING) {
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING
        } else {
            ElementSelectorFlags::empty()
        }
    }

    fn compute_layout_damage(_old: &ComputedValues, _new: &ComputedValues) -> RestyleDamage {
        // Chapters are styled once and laid out whole; incremental damage
        // tracking has no consumer.
        RestyleDamage::empty()
    }
}

/// The HTML "rules for parsing dimension values", via stylo's servo attr
/// helpers, packaged as a specified `<length-percentage>`.
fn parse_dimension_attr(value: &str) -> Option<style::values::specified::LengthPercentage> {
    use style::servo::attr::{parse_length, LengthOrPercentageOrAuto};
    use style::values::specified::{LengthPercentage, NoCalcLength, NoCalcPercentage};
    match parse_length(value) {
        LengthOrPercentageOrAuto::Length(length) => Some(LengthPercentage::Length(
            NoCalcLength::from_px(length.to_f32_px()),
        )),
        LengthOrPercentageOrAuto::Percentage(fraction) => Some(LengthPercentage::Percentage(
            NoCalcPercentage::new(fraction),
        )),
        LengthOrPercentageOrAuto::Auto => None,
    }
}

pub struct Traverser<'a> {
    parent: DomNode<'a>,
    child_index: usize,
}

impl<'a> Iterator for Traverser<'a> {
    type Item = DomNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = *self.parent.node().children.get(self.child_index)?;
        self.child_index += 1;
        Some(self.parent.with(id))
    }
}
