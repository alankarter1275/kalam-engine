import CChapbook
import Foundation

// The text surface as the platform's accessibility tree.
//
// A rasterized page is a picture, and a picture of text is unusable with
// a screen reader. Both halves of this file put `Session`'s text surface
// — `pageTextRuns()` for the lines, `speakablePage()` for the words,
// `rects(for:)` for the geometry — behind what the platform's reader
// actually walks: VoiceOver on iOS reads `UIAccessibilityElement`s, and
// VoiceOver on macOS asks an `NSAccessibility` static-text element the
// same range-and-extent questions Orca asks GTK's `AccessibleText`. The
// engine hands out runs; the platform wraps them in its own tree — the
// same split the GTK viewer proved against AT-SPI and the Android AAR
// carries as an `AccessibilityNodeProvider`.
//
// Geometry here assumes what the rest of this package assumes: page
// space (CSS px, page top-left) coincides with the host view's logical
// coordinates while the metrics carry no rotation, because the metrics
// were taken from the view's own bounds. A shell that rotates maps the
// rects itself, exactly as it maps its pixels.

#if canImport(UIKit) && !os(watchOS)
    import UIKit

    /// The current page as VoiceOver's element list — one static-text
    /// element per visual line, in reading order.
    ///
    /// A host view owes one delegation and one notification:
    ///
    /// ```swift
    /// let a11y = PageAccessibility(host: pageView, session: session)
    /// // after anything that may have replaced the page (post-render):
    /// a11y.pageChanged()
    /// ```
    ///
    /// `pageChanged` compares against the last seen page text, so calling
    /// it after every render is the intended pattern — a repaint no-ops, a
    /// turn rebuilds the elements and announces the new page so the turn
    /// is audible rather than discoverable. The elements' frames are
    /// container space, so they track the host through scrolls and layout
    /// without being rebuilt.
    @MainActor
    public final class PageAccessibility {
        private weak var host: UIView?
        private let session: Session
        /// Text last handed to VoiceOver — the change detector that lets
        /// `pageChanged` run after every render and speak only on turns.
        private var seen: String?

        public init(host: UIView, session: Session) {
            self.host = host
            self.session = session
            // A container of elements, never an element itself — an
            // element's children are unreachable to VoiceOver.
            host.isAccessibilityElement = false
        }

        /// Re-read the page if its text changed: rebuild the element
        /// list, tell VoiceOver the layout moved, and speak the new page.
        public func pageChanged() {
            guard let host else { return }
            let text = ((try? session.speakablePage()) ?? nil)?.text ?? ""
            if seen == text { return }
            seen = text
            let runs = ((try? session.pageTextRuns()) ?? nil) ?? []
            host.accessibilityElements = runs.map { run in
                let element = UIAccessibilityElement(accessibilityContainer: host)
                element.accessibilityLabel = run.text
                element.accessibilityTraits = .staticText
                element.accessibilityFrameInContainerSpace = run.rect
                return element
            }
            // The argument is the announcement; an empty page (a comic)
            // still reports the layout change so stale elements retract.
            UIAccessibility.post(
                notification: .layoutChanged, argument: text.isEmpty ? nil : text)
        }
    }
#endif

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
    import AppKit

    /// The current page as one macOS static-text accessibility element —
    /// the shape VoiceOver reads, navigates by word, and points at.
    ///
    /// One element for the whole page rather than one per line, because
    /// that is what `NSAccessibilityStaticText` is: VoiceOver does its own
    /// word and sentence walking over the string and comes back with
    /// range questions, which land on the word table and `rects(for:)` —
    /// the same accessor set GTK's `AccessibleText` wraps. A host view
    /// owes one delegation and one notification:
    ///
    /// ```swift
    /// let a11y = PageAccessibility(host: pageView, session: session)
    /// pageView.setAccessibilityChildren([a11y])
    /// // after anything that may have replaced the page (post-render):
    /// a11y.pageChanged()
    /// ```
    ///
    /// Offsets on this boundary are **UTF-16 code units**, because that is
    /// what `NSRange` means to AppKit — the engine's word table speaks
    /// Unicode scalars, and the conversion lives here so it exists exactly
    /// once. Geometry crosses in Cocoa screen coordinates (bottom-left
    /// origin); the flip is undone against the host view, whether or not
    /// the view itself is flipped.
    // Not a formal `NSAccessibilityStaticText` conformance: that protocol
    // redeclares `accessibilityValue()` as `-> String?` where the
    // superclass owns it as `-> Any?`, and Swift refuses the clash. The
    // accessibility runtime never checks the conformance — it asks
    // through the `NSAccessibilityProtocol` selectors overridden below,
    // steered by the `.staticText` role.
    public final class PageAccessibility: NSAccessibilityElement {
        private weak var host: NSView?
        private let session: Session
        /// The speakable string and word table as of the last
        /// `pageChanged`, plus the scalar→UTF-16 prefix map for it:
        /// `utf16Offsets[i]` is the UTF-16 offset of scalar `i`, with one
        /// extra entry for the end.
        private var text = ""
        private var words: [Session.WordSpan] = []
        private var utf16Offsets: [Int] = [0]
        private var seen: String?

        @MainActor
        public init(host: NSView, session: Session) {
            self.host = host
            self.session = session
            super.init()
            setAccessibilityRole(.staticText)
            setAccessibilityParent(host)
        }

        /// Re-read the page if its text changed: replace the string and
        /// word table, tell VoiceOver the value moved, and announce the
        /// new page so a turn is audible rather than discoverable. Safe
        /// (and intended) after every render — a repaint no-ops.
        @MainActor
        public func pageChanged() {
            let page = ((try? session.speakablePage()) ?? nil)
            let now = page?.text ?? ""
            if seen == now { return }
            seen = now
            text = now
            words = page?.words ?? []
            utf16Offsets = [0]
            utf16Offsets.reserveCapacity(text.unicodeScalars.count + 1)
            var total = 0
            for scalar in text.unicodeScalars {
                total += scalar.utf16.count
                utf16Offsets.append(total)
            }
            NSAccessibility.post(element: self, notification: .valueChanged)
            if !text.isEmpty {
                NSAccessibility.post(
                    element: self, notification: .announcementRequested,
                    userInfo: [
                        .announcement: text,
                        .priority: NSAccessibilityPriorityLevel.medium.rawValue,
                    ])
            }
        }

        // MARK: Offset conversion

        private var utf16Length: Int { utf16Offsets[utf16Offsets.count - 1] }

        private func utf16Offset(ofScalar index: UInt32) -> Int {
            utf16Offsets[min(Int(index), utf16Offsets.count - 1)]
        }

        /// The scalar whose UTF-16 span contains `offset` — clamped, and
        /// never mid-surrogate, because the prefix map has no entry there.
        private func scalarOffset(ofUTF16 offset: Int) -> UInt32 {
            var (lo, hi) = (0, utf16Offsets.count - 1)
            while lo < hi {
                let mid = (lo + hi + 1) / 2
                if utf16Offsets[mid] <= offset { lo = mid } else { hi = mid - 1 }
            }
            return UInt32(lo)
        }

        /// The locator range covered by the words overlapping a scalar
        /// range of the speakable text — how a text offset gets back to
        /// page geometry.
        private func locators(overlapping range: Range<UInt32>) -> Range<UInt32>? {
            let overlapping = words.filter {
                $0.textRange.lowerBound < range.upperBound
                    && range.lowerBound < $0.textRange.upperBound
            }
            guard let lo = overlapping.map(\.locators.lowerBound).min(),
                let hi = overlapping.map(\.locators.upperBound).max()
            else { return nil }
            return lo..<hi
        }

        // MARK: Geometry
        //
        // Static and `assumeIsolated` rather than `@MainActor` methods:
        // the nonisolated overrides below cannot hop actors, and the
        // system delivers accessibility queries on the main thread — the
        // assumption states that contract. Only the (Sendable, because
        // MainActor-bound) view crosses into the isolation, never `self`.

        /// Page space → Cocoa screen space, through the host view. Zero
        /// off-window, which is also what AppKit's own text views answer.
        private static func screenRect(fromPage rect: CGRect, in host: NSView?) -> NSRect {
            MainActor.assumeIsolated {
                guard let host, let window = host.window else { return .zero }
                let local =
                    host.isFlipped
                    ? rect
                    : NSRect(
                        x: rect.minX, y: host.bounds.height - rect.maxY,
                        width: rect.width, height: rect.height)
                return window.convertToScreen(host.convert(local, to: nil))
            }
        }

        private static func pagePoint(fromScreen point: NSPoint, in host: NSView?) -> CGPoint? {
            MainActor.assumeIsolated {
                guard let host, let window = host.window else { return nil }
                let local = host.convert(window.convertPoint(fromScreen: point), from: nil)
                return host.isFlipped
                    ? local
                    : CGPoint(x: local.x, y: host.bounds.height - local.y)
            }
        }

        // MARK: NSAccessibility
        //
        // The overrides are nonisolated because the superclass's are; see
        // the geometry note above for how the main-thread contract is
        // stated.

        public override func isAccessibilityElement() -> Bool { true }

        public override func accessibilityValue() -> Any? { text }

        public override func accessibilityVisibleCharacterRange() -> NSRange {
            NSRange(location: 0, length: utf16Length)
        }

        public override func accessibilityString(for range: NSRange) -> String? {
            guard let bounds = Range(range, in: text) else { return nil }
            return String(text[bounds])
        }

        public override func accessibilityAttributedString(for range: NSRange)
            -> NSAttributedString?
        {
            accessibilityString(for: range).map(NSAttributedString.init(string:))
        }

        public override func accessibilityFrame() -> NSRect {
            let host = self.host
            return MainActor.assumeIsolated {
                guard let host, let window = host.window else { return .zero }
                return window.convertToScreen(host.convert(host.bounds, to: nil))
            }
        }

        /// Screen-space extent of a UTF-16 range — the union of the lines
        /// its words touch, which is what VoiceOver draws its cursor
        /// around.
        public override func accessibilityFrame(for range: NSRange) -> NSRect {
            let scalars =
                scalarOffset(ofUTF16: range.location)..<scalarOffset(
                    ofUTF16: range.location + range.length)
            guard let locators = locators(overlapping: scalars),
                let rects = try? session.rects(for: locators),
                let union = rects.dropFirst().reduce(rects.first, { $0?.union($1) })
            else { return .zero }
            return Self.screenRect(fromPage: union, in: host)
        }

        /// The word under a screen point, as a UTF-16 range into the
        /// value — what a pointer hovering under VoiceOver reads.
        public override func accessibilityRange(for point: NSPoint) -> NSRange {
            guard let page = Self.pagePoint(fromScreen: point, in: host),
                let locators = ((try? session.word(at: page)) ?? nil),
                let word = words.first(where: { $0.locators == locators })
            else { return NSRange(location: 0, length: 0) }
            return NSRange(
                location: utf16Offset(ofScalar: word.textRange.lowerBound),
                length: utf16Offset(ofScalar: word.textRange.upperBound)
                    - utf16Offset(ofScalar: word.textRange.lowerBound))
        }
    }
#endif
