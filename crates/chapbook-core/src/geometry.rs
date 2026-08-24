//! Minimal f32 geometry in CSS-px space.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const ZERO: Size = Size { w: 0.0, h: 0.0 };

    pub fn new(w: f32, h: f32) -> Self {
        Size { w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        origin: Point::ZERO,
        size: Size::ZERO,
    };

    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect {
            origin: Point::new(x, y),
            size: Size::new(w, h),
        }
    }

    pub fn min_x(&self) -> f32 {
        self.origin.x
    }

    pub fn min_y(&self) -> f32 {
        self.origin.y
    }

    pub fn max_x(&self) -> f32 {
        self.origin.x + self.size.w
    }

    pub fn max_y(&self) -> f32 {
        self.origin.y + self.size.h
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.origin.x + dx,
            self.origin.y + dy,
            self.size.w,
            self.size.h,
        )
    }

    /// The smallest rect containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        let (x, y) = (
            self.min_x().min(other.min_x()),
            self.min_y().min(other.min_y()),
        );
        Rect::new(
            x,
            y,
            self.max_x().max(other.max_x()) - x,
            self.max_y().max(other.max_y()) - y,
        )
    }

    /// True if `other` lies entirely within `self`, with a small epsilon to
    /// absorb accumulated f32 layout error.
    pub fn contains_rect(&self, other: &Rect) -> bool {
        const EPS: f32 = 0.01;
        other.min_x() >= self.min_x() - EPS
            && other.min_y() >= self.min_y() - EPS
            && other.max_x() <= self.max_x() + EPS
            && other.max_y() <= self.max_y() + EPS
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.min_x() < other.max_x()
            && other.min_x() < self.max_x()
            && self.min_y() < other.max_y()
            && other.min_y() < self.max_y()
    }
}

/// Per-edge lengths (margins, padding, borders, page margins).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    pub fn uniform(v: f32) -> Self {
        EdgeSizes {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Non-premultiplied sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::new(0, 0, 0, 255);
    pub const WHITE: Rgba = Rgba::new(255, 255, 255, 255);
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }

    pub fn is_opaque(&self) -> bool {
        self.a == 255
    }

    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }
}
