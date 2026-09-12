# Architectural Restrictions & Guardrails

To ensure `kalam-engine` remains faithful to its core mission and never degenerates into a bloated web browser engine, every contributor and AI agent must strictly enforce the following rules.

---

## 🚫 1. Absolute Prohibition of C++ Web Engines (No WebKit, No Stylo)
- **NO `webkit2gtk` or WebViews:** `kalam-engine` MUST NEVER instantiate or depend on a browser engine.
- **NO `stylo` (Firefox CSS Engine):** Do not bring in `stylo` or Gecko C++ dependencies. `stylo` introduces massive compilation overhead and C++ toolchain requirements.
- **Rule:** If a feature cannot be rendered via pure-Rust text layout (`cosmic-text`), it must be stripped or simplified via `lol_html`.

---

## 🚫 2. No JavaScript & No JS Bridges
- `kalam-engine` operates entirely in compiled Rust code.
- Selection detection, cursor movement, word-hitting (`hit_test`), and pagination MUST be calculated in Rust.
- Never inject JavaScript scripts, V8/JSC runtimes, or IPC message handlers into the rendering pipeline.

---

## 🚫 3. No Feature Creep Beyond Engine Scope
- **NO Database Logic:** `kalam-engine` does NOT touch SQLite, database files, or library management. That is the responsibility of the host app (`kalam`).
- **NO Network / Scraping Logic:** `kalam-engine` does NOT make HTTP requests or parse online websites. It consumes local byte streams (EPUB files, HTML buffers).
- **NO Comic / PDF Engines:** `kalam-engine` focuses strictly on reflowable text. PDF rasterization and CBZ archive paging are handled by native GTK components in Kalam.

---

## 🔒 4. Strict Theme Obedience
- Publisher CSS rule overrides (e.g. fixed font-family, fixed text colors, fixed line heights, forced backgrounds) MUST be stripped by the `lol_html` sanitizer.
- The engine MUST enforce Kalam's active theme parameters (colors, font family, line height, paragraph spacing, margins) as authoritative.
