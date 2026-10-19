//! Bounded Type 3 outlines from the licensed regular Japanese document fallback.
use crate::{GlyphError, japanese_glyph};
use pdfcraft_cos::{Dict, Document, Object, Stream};

const MAX_TYPE3_GLYPHS: usize = 240;

#[derive(Clone)]
pub struct Type3Fallback {
    pub name: String,
    /// The family its glyphs come from ("Shippori Mincho").
    pub family: &'static str,
    pub codes: Vec<(char, u8, f64)>,
}

fn pdf_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

fn type3_path(ch: char) -> Result<(Vec<u8>, f64, [f64; 4]), String> {
    let glyph = japanese_glyph(ch).map_err(|e| match e {
        GlyphError::NoFont => no_japanese_font(),
        GlyphError::Missing => format!("Japanese fallback font has no glyph for U+{:04X}", ch as u32),
        GlyphError::TooComplex => format!("Japanese fallback glyph U+{:04X} is too complex", ch as u32),
    })?;
    let scale = 1000.0;
    let mut out = format!(
        "{} 0 {} {} {} {} d1\n",
        pdf_num(glyph.width * scale),
        pdf_num(glyph.bbox[0] * scale),
        pdf_num(glyph.bbox[1] * scale),
        pdf_num(glyph.bbox[2] * scale),
        pdf_num(glyph.bbox[3] * scale)
    )
    .into_bytes();
    for contour in glyph.contours {
        let Some(first) = contour.first() else { continue };
        out.extend_from_slice(format!("{} {} m\n", pdf_num(first[0] * scale), pdf_num(first[1] * scale)).as_bytes());
        for p in contour.iter().skip(1) {
            out.extend_from_slice(format!("{} {} l\n", pdf_num(p[0] * scale), pdf_num(p[1] * scale)).as_bytes());
        }
        out.extend_from_slice(b"h\n");
    }
    out.extend_from_slice(b"f\n");
    Ok((out, glyph.width, glyph.bbox))
}

fn unicode_hex(ch: char) -> String {
    let mut units = [0u16; 2];
    let encoded = ch.encode_utf16(&mut units);
    encoded.iter().map(|u| format!("{u:04X}")).collect()
}

/// This build has no Japanese face to draw replacement text with.
fn no_japanese_font() -> String {
    "this text needs PdfCraft's regular Japanese document font, which this build does not include; use an official release or build with CRAFT_FONTS_DIR pointing to craft-fonts".into()
}

pub fn type3_font(doc: &mut Document, fonts_res: &mut Dict, text: &str) -> Result<Type3Fallback, String> {
    let family = crate::document_japanese_font().ok_or_else(no_japanese_font)?.family;
    let mut chars = Vec::new();
    for ch in text.chars() {
        if !chars.contains(&ch) {
            if chars.len() >= MAX_TYPE3_GLYPHS {
                return Err("Japanese replacement has too many unique characters".to_string());
            }
            chars.push(ch);
        }
    }
    if chars.is_empty() {
        return Err("replacement text is empty".to_string());
    }
    // Resolve every outline before adding any object: unsupported text changes nothing.
    let paths: Vec<_> = chars.iter().map(|ch| type3_path(*ch)).collect::<Result<_, _>>()?;
    let mut codes = Vec::with_capacity(chars.len());
    let mut charprocs = Dict::new();
    let mut widths = vec![Object::Int(0); 242];
    let mut differences = Vec::new();
    let mut bbox = [0.0_f64; 4];
    let mut next_code = 1_u8;
    let mut last_code = 0_u8;
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n/CMapName /PCUnicode def\n/CIDSystemInfo << /Registry (PdfCraft) /Ordering (Unicode) /Supplement 0 >> def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    let count = chars.len();
    for (i, (ch, (path, width, glyph_bbox))) in chars.into_iter().zip(paths).enumerate() {
        // Tw applies to byte 32 even in Type 3 fonts. Never give that code to a CJK glyph.
        let code = if ch == ' ' {
            32
        } else {
            if next_code == 32 {
                next_code += 1;
            }
            let code = next_code;
            next_code += 1;
            code
        };
        last_code = last_code.max(code);
        bbox[0] = bbox[0].min(glyph_bbox[0]);
        bbox[1] = bbox[1].min(glyph_bbox[1]);
        bbox[2] = bbox[2].max(glyph_bbox[2]);
        bbox[3] = bbox[3].max(glyph_bbox[3]);
        let glyph_name = format!("g{code:02X}");
        let mut pd = Dict::new();
        pd.set(b"Length".to_vec(), path.len() as i64);
        let proc_ref = doc.add(Object::Stream(Stream::from_raw(pd, path)));
        charprocs.set(glyph_name.as_bytes().to_vec(), Object::Ref(proc_ref));
        differences.push(Object::Int(i64::from(code)));
        differences.push(Object::name(&glyph_name));
        if let Some(slot) = widths.get_mut(usize::from(code) - 1) {
            *slot = Object::Real((width * 1000.0).round());
        }
        if i % 100 == 0 {
            cmap.push_str(&format!("{} beginbfchar\n", (count - i).min(100)));
        }
        cmap.push_str(&format!("<{code:02X}> <{}>\n", unicode_hex(ch)));
        if i % 100 == 99 || i + 1 == count {
            cmap.push_str("endbfchar\n");
        }
        codes.push((ch, code, width));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut cmap_dict = Dict::new();
    cmap_dict.set(b"Length".to_vec(), cmap.len() as i64);
    let cmap_ref = doc.add(Object::Stream(Stream::from_raw(cmap_dict, cmap.into_bytes())));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"Name".to_vec(), Object::name("PCJapanese"));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(v * 1000.0)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(i64::from(last_code)));
    widths.truncate(usize::from(last_code));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    let mut name = String::from("PCJp");
    let mut suffix = 0usize;
    while fonts_res.contains(name.as_bytes()) {
        suffix = suffix.saturating_add(1);
        name = format!("PCJp{suffix}");
    }
    fonts_res.set(name.as_bytes().to_vec(), Object::Dict(font));
    Ok(Type3Fallback { name, family, codes })
}

pub fn type3_encode(fallback: &Type3Fallback, text: &str) -> Option<Vec<u8>> {
    text.chars().map(|ch| fallback.codes.iter().find(|(c, _, _)| *c == ch).map(|(_, code, _)| *code)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_preflight_never_leaves_partial_font_objects() {
        let mut doc = Document::new_empty();
        let before = pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap();
        let mut resources = Dict::new();
        assert!(type3_font(&mut doc, &mut resources, "\u{65e5}\u{1f4a9}").is_err());
        assert!(resources.is_empty());
        assert_eq!(pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap(), before);
    }

    #[test]
    fn fallback_codes_do_not_apply_word_spacing_to_nonspaces() {
        if crate::document_japanese_font().is_none() {
            return;
        }
        let text: String = (0x3041..=0x3065).filter_map(char::from_u32).collect();
        let mut doc = Document::new_empty();
        let mut resources = Dict::new();
        let font = type3_font(&mut doc, &mut resources, &text).unwrap();
        assert!(font.codes.iter().all(|(_, code, _)| *code != 32));
        let metrics = crate::pdf::Metrics::from_dict(&doc, resources.get(font.name.as_bytes()).unwrap().as_dict().unwrap());
        assert_eq!(metrics.decode(&type3_encode(&font, &text).unwrap()), text);
    }
}
