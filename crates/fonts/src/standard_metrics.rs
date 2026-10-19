//! Exact advance data for the twelve Latin standard PDF faces.
//! Generated in OUT_DIR from hayro-interpret 0.7.0 src/font/generated/metrics.rs,
//! with base encodings from generated/{win_ansi,mac_roman,standard}.rs,
//! revision 0c98904d2ced8e37b63ed2bd1e60f95bc0c7c774.
//! Source SHA-256: 2ec9e608573eda459de1e1d9bda3bb494315e36bee885acc88912a11fe0caa41.
//! Nonvisual Core14 metric data permitted by AGENTS.md §1.1; no outlines/font programs.
//! Copyright (c) The Hayro Authors. Used under MIT; see ../LICENSE-hayro-metrics.

/// An exact advance in text space, indexed by the selected PostScript glyph name.
pub(crate) fn width(base: &[u8], glyph: &[u8]) -> Option<f64> {
    let face = match base {
        b"Courier" => 0,
        b"Courier-Bold" => 1,
        b"Courier-Oblique" => 2,
        b"Courier-BoldOblique" => 3,
        b"Helvetica" => 4,
        b"Helvetica-Bold" => 5,
        b"Helvetica-Oblique" => 6,
        b"Helvetica-BoldOblique" => 7,
        b"Times-Roman" => 8,
        b"Times-Bold" => 9,
        b"Times-Italic" => 10,
        b"Times-BoldItalic" => 11,
        _ => return None,
    };
    // PDF base encodings use these aliases for the same standard-face outlines.
    let glyph = match glyph {
        b"nbspace" => b"space".as_slice(),
        b"sfthyphen" => b"hyphen".as_slice(),
        _ => glyph,
    };
    let index = LATIN_ADVANCES.binary_search_by(|(name, _)| name.as_bytes().cmp(glyph)).ok()?;
    LATIN_ADVANCES.get(index)?.1.get(face).map(|width| f64::from(*width) / 1000.0)
}

/// The actual glyph names selected by each supported base encoding.
pub(crate) fn base_glyphs(name: &[u8]) -> &'static [&'static str; 256] {
    match name {
        b"WinAnsiEncoding" => &WIN_ANSI,
        b"MacRomanEncoding" => &MAC_ROMAN,
        _ => &STANDARD,
    }
}

// Constants are derived at build time from the existing approved vendor tables.
include!(concat!(env!("OUT_DIR"), "/standard_metrics_data.rs"));

#[cfg(test)]
mod tests {
    use crate::{generated::standard_font, pdf::Metrics};
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    #[test]
    fn exact_standard_faces_keep_encoding_geometry_and_explicit_width_precedence() {
        let mut doc = Document::new_empty();
        // Independent standard-font constants, not values read back from LATIN_ADVANCES.
        for (base, o, i, a) in [
            ("Courier", 600.0, 600.0, 600.0),
            ("Courier-Bold", 600.0, 600.0, 600.0),
            ("Courier-Oblique", 600.0, 600.0, 600.0),
            ("Courier-BoldOblique", 600.0, 600.0, 600.0),
            ("Helvetica", 778.0, 278.0, 667.0),
            ("Helvetica-Bold", 778.0, 278.0, 722.0),
            ("Helvetica-Oblique", 778.0, 278.0, 667.0),
            ("Helvetica-BoldOblique", 778.0, 278.0, 722.0),
            ("Times-Roman", 722.0, 333.0, 722.0),
            ("Times-Bold", 778.0, 389.0, 722.0),
            ("Times-Italic", 722.0, 333.0, 611.0),
            ("Times-BoldItalic", 722.0, 389.0, 667.0),
        ] {
            let font = standard_font(base);
            let metrics = Metrics::from_dict(&doc, font.as_dict().unwrap());
            assert!((metrics.width(u32::from(b'O')) * 1000.0 - o).abs() < 1e-8, "{base}");
            assert!((metrics.width(u32::from(b'I')) * 1000.0 - i).abs() < 1e-8, "{base}");
            assert!((metrics.width(u32::from(b'A')) * 1000.0 - a).abs() < 1e-8, "{base}");
        }
        let Object::Dict(mut font) = standard_font("Helvetica") else { panic!() };
        let mut encoding = Dict::new();
        encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("Abreve")]));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        let cmap = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1 beginbfchar <41> <0049> endbfchar".to_vec())));
        font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.decode(b"A"), "I");
        assert!((metrics.width(65) - 0.667).abs() < 1e-8, "Abreve geometry is independent of extraction and the local Unicode name table");
        font.set(b"FirstChar".to_vec(), Object::Int(65));
        font.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(123)]));
        assert!((Metrics::from_dict(&doc, &font).width(65) - 0.123).abs() < 1e-8);
        let mut font = standard_font("Helvetica").as_dict().unwrap().clone();
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.width(160), metrics.width(32), "WinAnsi nonbreaking space selects space");
        assert_eq!(metrics.width(173), metrics.width(45), "WinAnsi soft hyphen selects hyphen");
        assert!((metrics.width(39) - 0.191).abs() < 1e-8, "WinAnsi selects quotesingle");
        font.set(b"Encoding".to_vec(), Object::name("MacRomanEncoding"));
        assert!((Metrics::from_dict(&doc, &font).width(128) - 0.667).abs() < 1e-8, "MacRoman selects Adieresis");
        font.remove(b"Encoding");
        assert!((Metrics::from_dict(&doc, &font).width(39) - 0.222).abs() < 1e-8, "built-in StandardEncoding selects quoteright");
    }
}
