//! Bounded Type 3 outlines from the licensed regular Japanese document fallback.
use crate::{GlyphError, japanese_glyph};
use pdfcraft_cos::{Dict, Document, Object, Stream};
use skrifa::{
    FontRef, MetadataProvider,
    attribute::Style,
    instance::{LocationRef, Size},
};

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
    "this text needs PdfCraft's Japanese fallback font (regular document face), which this build does not include; use an official release or build with CRAFT_FONTS_DIR pointing to craft-fonts".into()
}

pub fn type3_font(doc: &mut Document, fonts_res: &mut Dict, text: &str) -> Result<Type3Fallback, String> {
    let source = crate::document_japanese_font().ok_or_else(no_japanese_font)?;
    let family = source.family;
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
    let face = FontRef::new(source.bytes).map_err(|_| "Japanese fallback font metadata could not be read".to_string())?;
    let source_metrics = face.metrics(Size::new(1000.0), LocationRef::default());
    let italic = !matches!(face.attributes().style, Style::Normal);

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
    // Type3 has no BaseFont. Its own Name and descriptor FontName identify this generated
    // face/bank, rather than pretending it is the source font program. These fields predate
    // PDF 1.5's FontFamily, so reporting the identity does not require a version upgrade.
    let font_name =
        format!("{}-{}-Type3-{}", family.split_whitespace().collect::<String>(), source.style.split_whitespace().collect::<String>(), cmap_ref.num);
    font.set(b"Name".to_vec(), Object::name(&font_name));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&font_name));
    // Symbolic custom encoding; slope/monospace describe the actual outline source, not
    // the original PDF's requested style. No synthetic bold or italic is implied.
    descriptor.set(b"Flags".to_vec(), Object::Int(4 | if italic { 64 } else { 0 } | if source_metrics.is_monospace { 1 } else { 0 }));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Real(f64::from(source_metrics.italic_angle)));
    if let Some(height) = source_metrics.cap_height {
        descriptor.set(b"CapHeight".to_vec(), Object::Real(f64::from(height)));
    }
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
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

    #[test]
    fn generated_type3_banks_have_truthful_persistent_names_without_version_upgrade() {
        if crate::SHIPPORI_MINCHO.is_none() {
            eprintln!("requires the optional Shippori Mincho release font");
            return;
        }
        let mut bytes = pdfcraft_cos::write_full(&Document::new_empty(), &Default::default()).unwrap();
        bytes[5..8].copy_from_slice(b"1.3");
        let mut doc = Document::open(std::sync::Arc::new(bytes)).unwrap();
        let mut resources = Dict::new();
        let mut names = Vec::new();
        for text in ["A日", "B本"] {
            let bank = type3_font(&mut doc, &mut resources, text).unwrap();
            let font = resources.get(bank.name.as_bytes()).unwrap().as_dict().unwrap();
            assert!(!font.contains(b"BaseFont"));
            let descriptor_ref = font.reference(b"FontDescriptor").expect("descriptor must be indirect before PDF 2.0");
            let descriptor = doc.get(descriptor_ref);
            let descriptor = descriptor.as_dict().unwrap();
            assert!(!descriptor.contains(b"FontFamily"));
            assert_eq!(descriptor.name(b"FontName"), font.name(b"Name"));
            let metrics = crate::pdf::Metrics::from_dict(&doc, font);
            let bank_id = metrics.base_font.strip_prefix("ShipporiMincho-Regular-Type3-").expect("actual release face identity");
            assert!(!bank_id.is_empty() && bank_id.bytes().all(|b| b.is_ascii_digit()));
            assert!(!metrics.bold && !metrics.italic);
            names.push(metrics.base_font);
        }
        assert_ne!(names[0], names[1], "separate glyph banks need separate identities");
        assert_eq!(doc.version(), "1.3");
        assert!(!doc.get(doc.root().unwrap()).as_dict().unwrap().contains(b"Version"));
    }
}
