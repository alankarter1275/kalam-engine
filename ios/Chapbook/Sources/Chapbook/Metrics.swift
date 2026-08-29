import CChapbook
import Foundation

/// Quarter-turns clockwise between the page as laid out and the panel it
/// is painted into. A property of the output, never of the layout.
public enum Rotation: UInt32, Sendable {
    case none = 0
    case quarter = 1
    case half = 2
    case threeQuarter = 3
}

/// The page box in logical units (points, on Apple platforms), plus the
/// scale that turns it into device pixels.
///
/// **`width` and `height` are the page in reading orientation, not the
/// panel.** They coincide only while `rotation` is `.none`; on a quarter
/// turn the axes swap, so a 600x800 view wanting a turned page passes
/// 800x600 here and gets 600x800 back from [`Session.renderSize()`].
/// Getting this backwards is not an error anything can catch — the page
/// just paginates to the wrong aspect — which is why it is a sentence
/// here rather than a status code there.
public struct PageMetrics: Sendable {
    public var width: CGFloat
    public var height: CGFloat
    public var marginTop: CGFloat
    public var marginRight: CGFloat
    public var marginBottom: CGFloat
    public var marginLeft: CGFloat
    /// `UIScreen.scale` / the view's `contentScaleFactor`.
    public var dpiScale: CGFloat
    public var rotation: Rotation

    public init(
        width: CGFloat,
        height: CGFloat,
        marginTop: CGFloat = 40,
        marginRight: CGFloat = 24,
        marginBottom: CGFloat = 40,
        marginLeft: CGFloat = 24,
        dpiScale: CGFloat,
        rotation: Rotation = .none
    ) {
        self.width = width
        self.height = height
        self.marginTop = marginTop
        self.marginRight = marginRight
        self.marginBottom = marginBottom
        self.marginLeft = marginLeft
        self.dpiScale = dpiScale
        self.rotation = rotation
    }

    var raw: cb_metrics {
        cb_metrics(
            width: Float(width), height: Float(height),
            margin_top: Float(marginTop), margin_right: Float(marginRight),
            margin_bottom: Float(marginBottom), margin_left: Float(marginLeft),
            dpi_scale: Float(dpiScale), rotation: rotation.rawValue)
    }
}

/// Page ground and default text colours.
public enum Theme: UInt32, Sendable {
    case light = 0
    case sepia = 1
    case dark = 2
}

/// How a page is typeset. `baseFontSize` and `lineHeight` are the two a
/// reader adjusts; the rest an app usually sets once.
///
/// Dynamic Type is deliberately the app's to honour: read
/// `UIContentSizeCategory`, map it onto `baseFontSize`, and the engine
/// keeps the reader's place across the reflow.
public struct ReadingSettings: Sendable {
    public var baseFontSize: CGFloat
    /// Unitless multiplier.
    public var lineHeight: CGFloat
    public var justify: Bool
    /// Honour the publisher's stylesheets. Off leaves UA and user sheets.
    public var publisherStyles: Bool
    public var theme: Theme

    init(raw: cb_settings) {
        baseFontSize = CGFloat(raw.base_font_px)
        lineHeight = CGFloat(raw.line_height)
        justify = raw.justify
        publisherStyles = raw.publisher_styles
        theme = Theme(rawValue: raw.theme) ?? .light
    }

    var raw: cb_settings {
        cb_settings(
            base_font_px: Float(baseFontSize), line_height: Float(lineHeight),
            justify: justify, publisher_styles: publisherStyles,
            theme: theme.rawValue)
    }
}

/// Where a settings change sticks.
public enum SettingsScope: UInt32, Sendable {
    /// Every book without an override of its own.
    case global = 0
    /// The open book only.
    case thisBook = 1
}
