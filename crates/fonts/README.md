# pdfcraft-fonts

Layer L2. Font helpers for the appearance streams PdfCraft generates (comments, form fields).

Today it holds what generated appearances need without a font program:

- `helvetica_width`: a legacy approximation of Helvetica's proportions by character class.
  It requires no font program. Generated appearances use the selected font's
  metrics instead: exact approved advance tables for the twelve standard Latin PDF faces,
  and explicit PDF widths where supplied. Unknown faces retain the existing approximation.
- `wrap`: greedy line breaking with that measure (paragraphs on newlines, long words split).
- `win_ansi`: Unicode → WinAnsiEncoding bytes (`?` for characters it can't represent), and
  `literal` to write bytes as a PDF literal string.

It also holds the fonts of the optional [craft-fonts](https://github.com/storytold/craft-fonts)
build input. `build.rs` embeds them as `CRAFT_FONTS` when the build sets
`CRAFT_FONTS_DIR=<checkout>` (and fails if it can't while `CRAFT_FONTS_REQUIRED=1`, as releases
set). Without that variable `CRAFT_FONTS` is empty and everything below copes:

- `ui_japanese_fonts`: the `Jpan` faces for the interface, BIZ UDPGothic first.
- `ui_chinese_fonts`: the `Hans` faces for the interface, in manifest order (without one,
  Chinese characters the Japanese faces lack show the replacement glyph).
- `ui_cjk_fonts(prefer_hans)`: both in fallback order for the UI language (Chinese group first
  in Chinese mode, so one line never mixes faces with different vertical metrics).
- `document_japanese_font` / `japanese_glyph`: the face (Shippori Mincho, then BIZ UDMincho) whose
  outlines become the Type 3 fallback font for Japanese text written into PDFs. Without it,
  `japanese_glyph` returns `GlyphError::NoFont` and the editor reports a clear error.
- `SHIPPORI_MINCHO`: Shippori Mincho's bytes, or `None`.

wasm32 builds embed only BIZ UDPGothic Regular, to keep the web build small: the web build
currently has no Chinese face, so Chinese there still shows the replacement glyph.
Font files are never
committed here (`AGENTS.md` §1.4; team members: [craftrules `standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md), internal).

The full font subsystem (parsing, shaping, subsetting and embedding) arrives with M2.2/M7.

Generated page text and form appearances use `generated::TextFonts`: the primary font's actual
encoding, then the existing regular Japanese document face for uncovered CJK glyphs, with
ToUnicode mappings. Unsupported characters, unavailable fonts, and unavailable CJK bold/italic
styles return actionable errors; they are never replaced with question marks. Preparation is
bounded to 65536 input characters and 240 distinct fallback glyphs. This does not add shaping,
bidirectional text, arbitrary-script fallback, or complete Chinese coverage. Latin keeps its
primary face; Japanese fallback follows the existing `document_japanese_font` selection
(Shippori Mincho preferred).

Generated Type0 text accepts Identity-H and explicit fixed-length horizontal embedded CMaps
with CID mappings. Vertical, mixed-length, inherited and unknown predefined CMaps return an
error without changing the document; they require layout/encoding support not implemented here.

Generated Type3 banks record their actual fallback face and bank in the standard FontDescriptor
FontName (for example, `ShipporiMincho-Regular-Type3-42`). Editing and inspection tools report
that identity after saving/reopening; unnamed imported fonts are not assigned a guessed family.
This uses fields supported before PDF 1.5 and does not raise the document version. It does not
add Japanese bold/italic faces or preserve an unavailable source sans/serif style.

The standard Latin advance tables and base encodings are generated in Cargo's `OUT_DIR`
by `build.rs` from the already attributed `vendor/hayro-interpret` 0.7.0 Core14 data. No
second copy of the data is tracked. The MIT notice is retained in `LICENSE-hayro-metrics`
and the original vendor licence files; the existing bundled attribution remains authoritative.
The extraction is deterministic and tracks each vendor input for rebuilds. It adds no runtime
dependency or font program. Geometry follows encoding glyph names before ToUnicode extraction
mappings, and explicit PDF `/Widths` remain authoritative.
