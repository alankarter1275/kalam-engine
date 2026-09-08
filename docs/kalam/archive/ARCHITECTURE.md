# Technical Architecture & Pipeline

`kalam-engine` is designed around a clean, multi-stage pipeline that converts raw EPUB container bytes into rendered GTK surfaces and interactive text hits.

```
[ EPUB Archive / HTML Input ]
             │
             ▼
    1. Container & OPF Parser (`quick-xml`, `zip`)
             │
             ▼
    2. HTML Normalizer & CSS Stripper (`lol_html`)
             │
             ▼
    3. Text Layout & Shaping (`cosmic-text`)
             │
             ▼
    4. Discrete Page Splitting & DisplayList Generation
             │
             ▼
    5. Surface Painter (`tiny-skia` / `gtk4::DrawingArea`)
             │
             ▼
    6. Selection & Quote-Anchored Locator (`LayeredLocator`)
```

---

## Stage 1: Container & OPF Parser
- Extracts `container.xml` and locates the root package file (`.opf`).
- Reads the manifest items and spine order.
- Provides a clean sequential iterator over EPUB chapters.

## Stage 2: Normalization (`lol_html`)
- Intercepts raw HTML strings using Cloudflare's streaming `lol_html` parser.
- Strips inline `style="..."` attributes, `<style>` tags, and publisher CSS links.
- Preserves semantic markup (`<h1>`-`<h6>`, `<p>`, `<strong>`, `<em>`, `<blockquote>`, `<img>`).
- Wraps normalized tokens into a clean structured DOM/AST.

## Stage 3: Layout & Shaping (`cosmic-text`)
- Uses `cosmic-text` font fallback and shaping (`swash`/`harfbuzz`).
- Calculates glyph positions, line boundaries, and word bounds based on target viewport dimensions and theme parameters (font size, line spacing, margins).

## Stage 4: Pagination & DisplayList
- Computes height boundaries per page.
- Emits a immutable `DisplayList` representing glyph render commands, image draw rects, and highlight overlays for each page index.
- Turning pages is an `O(1)` slice switch on the `DisplayList` array.

## Stage 5: GTK Surface Painting
- Implements a custom GTK widget wrapper around `gtk4::DrawingArea`.
- Uses `tiny-skia` to render the active `DisplayList` directly onto GTK Cairo/GSK render nodes.

## Stage 6: Quote-Anchored Locators (`LayeredLocator`)
- Adapted from Chapbook's locator architecture.
- Stores position as:
  ```rust
  pub struct LayeredLocator {
      pub chapter_href: String,
      pub text_quote: String,     # E.g. "It was the best of times"
      pub dom_path: String,       # XPath / CSS selector anchor
      pub percentage: f32,        # Fallback 0.0 - 1.0 progress
  }
  ```
- Guarantees bookmarks, highlights, and dictionary lookups stay rock-solid across font size changes and window resizes.
