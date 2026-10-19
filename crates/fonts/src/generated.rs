//! Text for generated content: retain the primary encoding and use the existing regular
//! Japanese document face only for uncovered CJK characters. This is not a shaping engine.

use crate::{Type3Fallback, literal, pdf::Metrics, type3_font};
use pdfcraft_cos::{Dict, Document, Object};
use std::collections::HashMap;

pub struct TextFonts {
    primary: String,
    primary_codes: HashMap<char, (Vec<u8>, f64)>,
    fallback: Option<Type3Fallback>,
    pub fonts: Dict,
}

pub fn standard_font(base: &str) -> Object {
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name(base));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    Object::Dict(font)
}

fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x30ff | 0x31f0..=0x31ff | 0x3400..=0x9fff | 0xf900..=0xfaff | 0xff00..=0xffef | 0x20000..=0x323af)
}

impl TextFonts {
    /// Preflight every character, then install bounded outlines. Newline characters are
    /// layout separators; callers must split lines before `show`. Missing coverage is an error.
    pub fn new(doc: &mut Document, name: &str, primary: Object, text: &str) -> Result<Self, String> {
        Self::new_in(doc, name, primary, text, &Dict::new())
    }

    /// Reserve names in the actual destination resource dictionary, including imported names
    /// that happen to contain a newly allocated object number.
    pub fn new_in(doc: &mut Document, name: &str, primary: Object, text: &str, reserved: &Dict) -> Result<Self, String> {
        let mut name = name.to_string();
        while reserved.contains(name.as_bytes()) {
            name.push('_');
        }
        if text.chars().take(65_537).count() > 65_536 {
            return Err("generated text exceeds 65536 characters; split it into smaller items".into());
        }
        let resolved = doc.resolve(&primary);
        let dict = resolved.as_dict().ok_or_else(|| "the selected text font is not a dictionary".to_string())?;
        crate::pdf::validate_generated_encoding(doc, dict)?;
        let metrics = Metrics::from_dict(doc, dict);
        let mut missing = String::new();
        let mut primary_codes = HashMap::new();
        for ch in text.chars().filter(|c| !matches!(c, '\n' | '\r')) {
            if primary_codes.contains_key(&ch) || missing.contains(ch) {
                continue;
            }
            if let Some(bytes) = metrics.encode_char(ch) {
                let width = metrics.codes(&bytes).iter().map(|(code, _)| metrics.width(*code)).sum();
                primary_codes.insert(ch, (bytes, width));
                continue;
            }
            if !cjk(ch) {
                return Err(format!(
                    "the selected font cannot encode U+{:04X}; choose a font containing this character (automatic fallback supports CJK only)",
                    ch as u32
                ));
            }
            if metrics.bold || metrics.italic {
                return Err(
                    "CJK fallback has only a regular document face; choose regular text or a font containing these characters in the requested style"
                        .into(),
                );
            }
            if missing.chars().count() >= 240 {
                return Err("CJK text has more than 240 distinct fallback characters; split it into smaller items".into());
            }
            missing.push(ch);
        }
        let mut fonts = Dict::new();
        fonts.set(name.as_bytes().to_vec(), primary);
        let fallback = if missing.is_empty() { None } else { Some(type3_font(doc, &mut fonts, &missing)?) };
        // Each generated font owns its codes; use an object-specific resource name so adding
        // another item to the page cannot change the meaning of an earlier item's bytes.
        let fallback = fallback.map(|mut fallback| {
            if let Some(font) = fonts.remove(fallback.name.as_bytes()) {
                let object = doc.add(font);
                fallback.name = format!("PCUnicode{}", object.num);
                while fonts.contains(fallback.name.as_bytes()) || reserved.contains(fallback.name.as_bytes()) {
                    fallback.name.push('_');
                }
                fonts.set(fallback.name.as_bytes().to_vec(), Object::Ref(object));
            }
            fallback
        });
        Ok(Self { primary: name, primary_codes, fallback, fonts })
    }

    pub fn primary_name(&self) -> &str {
        &self.primary
    }

    pub fn width(&self, text: &str, size: f64) -> f64 {
        text.chars()
            .map(|ch| {
                if let Some((_, width)) = self.primary_codes.get(&ch) {
                    *width
                } else {
                    self.fallback.as_ref().and_then(|f| f.codes.iter().find(|(c, _, _)| *c == ch)).map_or(0.0, |(_, _, width)| *width)
                }
            })
            .sum::<f64>()
            * size
    }

    /// Emit font switches only at encoding boundaries, retaining Latin styling and spacing.
    pub fn show(&self, text: &str, size: f64) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut bytes = Vec::new();
        let mut previous = None;
        let flush = |out: &mut Vec<u8>, bytes: &mut Vec<u8>, fallback: bool| {
            let name = if fallback { self.fallback.as_ref().map_or(self.primary.as_str(), |f| f.name.as_str()) } else { &self.primary };
            out.extend(format!("/{name} {size:.4} Tf ").bytes());
            out.extend(literal(bytes));
            out.extend_from_slice(b" Tj\n");
            bytes.clear();
        };
        for ch in text.chars() {
            let (fallback, encoded) = match self.primary_codes.get(&ch) {
                Some((bytes, _)) => (false, bytes.clone()),
                None => {
                    let code = self
                        .fallback
                        .as_ref()
                        .and_then(|f| f.codes.iter().find(|(c, _, _)| *c == ch))
                        .map(|(_, code, _)| *code)
                        .ok_or_else(|| format!("text was not prepared for U+{:04X}", ch as u32))?;
                    (true, vec![code])
                }
            };
            if let Some(last) = previous
                && last != fallback
            {
                flush(&mut out, &mut bytes, last);
            }
            previous = Some(fallback);
            bytes.extend(encoded);
        }
        flush(&mut out, &mut bytes, previous.unwrap_or(false));
        Ok(out)
    }

    pub fn wrap(&self, text: &str, size: f64, width: f64) -> Vec<String> {
        let mut lines = Vec::new();
        let space = self.width(" ", size);
        for para in crate::text_lines(text) {
            let mut line = String::new();
            let mut line_width = 0.0;
            for word in para.split(' ') {
                let word_width = self.width(word, size);
                let candidate_width = if line.is_empty() { word_width } else { line_width + space + word_width };
                if candidate_width <= width {
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(word);
                    line_width = candidate_width;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                line_width = 0.0;
                for ch in word.chars() {
                    let advance = self.width(&ch.to_string(), size);
                    if !line.is_empty() && line_width + advance > width {
                        lines.push(std::mem::take(&mut line));
                        line_width = 0.0;
                    }
                    line.push(ch);
                    line_width += advance;
                }
            }
            lines.push(line);
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_winansi_mapping_keeps_characters_the_lossy_helper_replaced() {
        let mut doc = Document::new_empty();
        let text = "A \u{0160}\u{0153}\u{0178} \u{20ac}";
        let fonts = TextFonts::new(&mut doc, "F", standard_font("Helvetica"), text).unwrap();
        assert!(fonts.fallback.is_none());
        let metrics = Metrics::from_dict(&doc, doc.resolve(fonts.fonts.get(b"F").unwrap()).as_dict().unwrap());
        let bytes: Vec<u8> = text.chars().flat_map(|c| fonts.primary_codes.get(&c).unwrap().0.clone()).collect();
        assert_eq!(metrics.decode(&bytes), text);
        assert!(!bytes.contains(&b'?'));
    }

    #[test]
    fn standard_font_advances_follow_encoding_glyphs_not_code_numbers_or_to_unicode() {
        let mut doc = Document::new_empty();
        let Object::Dict(mut font) = standard_font("Helvetica") else { panic!() };
        let check = |doc: &mut Document, font: &Dict, shown: &[u8], advances: [f64; 2]| {
            let fonts = TextFonts::new(doc, "F", Object::Dict(font.clone()), "WI").unwrap();
            assert_eq!(fonts.show("WI", 12.0).unwrap(), shown);
            // Independent constants from the existing approximate Helvetica metric contract.
            assert!((fonts.width("W", 12.0) - advances[0]).abs() < 1e-8);
            assert!((fonts.width("I", 12.0) - advances[1]).abs() < 1e-8);
            assert!(fonts.fallback.is_none());
        };
        check(&mut doc, &font, b"/F 12.0000 Tf (WI) Tj\n", [11.328, 3.12]);

        // Original fixture: code 73 paints W; code 87 paints I.
        let mut encoding = Dict::new();
        encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(73), Object::name("W"), Object::Int(87), Object::name("I")]));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        check(&mut doc, &font, b"/F 12.0000 Tf (IW) Tj\n", [11.328, 3.12]);
        let fonts = TextFonts::new(&mut doc, "F", Object::Dict(font.clone()), "WW").unwrap();
        assert_eq!(fonts.wrap("WW", 12.0, 15.0), ["W", "W"]);

        // ToUnicode can give glyphs different extraction meanings without changing their shape.
        let cmap = b"2 beginbfchar <49> <0049> <57> <0057> endbfchar";
        font.set(b"ToUnicode".to_vec(), Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), cmap.to_vec())))));
        check(&mut doc, &font, b"/F 12.0000 Tf (WI) Tj\n", [3.12, 11.328]);

        // Explicit per-code widths retain precedence over standard-font approximations.
        font.remove(b"ToUnicode");
        font.set(b"FirstChar".to_vec(), Object::Int(73));
        let mut widths = vec![Object::Int(700); 15];
        widths[14] = Object::Int(200);
        font.set(b"Widths".to_vec(), Object::Array(widths));
        check(&mut doc, &font, b"/F 12.0000 Tf (IW) Tj\n", [8.4, 2.4]);
    }

    fn composite_font(doc: &mut Document, encoding: Object, source: &str) -> Object {
        let mut descendant = Dict::new();
        descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        descendant.set(b"DW".to_vec(), Object::Int(1000));
        descendant.set(b"W".to_vec(), Object::Array(vec![Object::Int(7), Object::Array(vec![Object::Int(600)])]));
        let unicode = format!("1 beginbfchar <{source}> <0041> endbfchar");
        let cmap = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), unicode.into_bytes())));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type0"));
        font.set(b"Encoding".to_vec(), encoding);
        font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
        font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)]));
        Object::Dict(font)
    }

    #[test]
    fn horizontal_fixed_length_composite_codes_keep_exact_bytes_and_advances() {
        for (encoding, source, bytes) in [
            (None, "0007", vec![0, 7]),
            (Some("1 begincodespacerange <00> <7F> endcodespacerange 1 begincidchar <41> 7 endcidchar"), "41", vec![0x41]),
            (Some("/WMode 0 def 1 begincodespacerange <0000> <00FF> endcodespacerange 1 begincidchar <0041> 7 endcidchar"), "0041", vec![0, 0x41]),
        ] {
            let mut doc = Document::new_empty();
            let encoding = encoding.map_or_else(
                || Object::name("Identity-H"),
                |program| Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), program.as_bytes().to_vec())))),
            );
            let font = composite_font(&mut doc, encoding, source);
            let text = TextFonts::new(&mut doc, "F", font, "A").unwrap();
            assert_eq!(text.primary_codes.get(&'A').unwrap().0, bytes);
            assert!((text.width("A", 12.0) - 7.2).abs() < 1e-8);
            assert!(text.fallback.is_none());
        }
    }

    #[test]
    fn composite_font_validation_rejects_unproved_or_vertical_programs() {
        let base = "1 begincodespacerange <00> <7F> endcodespacerange 1 begincidchar <41> 7 endcidchar";
        for prefix in ["/WMode 1 def", "/WM#6fde 1 def", "/Other usecmap", "/WMode 0 1 add def"] {
            let mut doc = Document::new_empty();
            let program = format!("{prefix} {base}");
            let encoding = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), program.into_bytes()))));
            let font = composite_font(&mut doc, encoding, "41");
            assert!(TextFonts::new(&mut doc, "F", font, "A").is_err(), "{prefix}");
        }
        let mut doc = Document::new_empty();
        let mut dict = Dict::new();
        dict.set(b"UseCMap".to_vec(), Object::name("Identity-H"));
        let encoding = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(dict, base.as_bytes().to_vec()))));
        let font = composite_font(&mut doc, encoding, "41");
        assert!(TextFonts::new(&mut doc, "F", font, "A").is_err());
    }

    #[test]
    fn unicode_roundtrip_does_not_authorize_an_extra_unknown_code() {
        let mut doc = Document::new_empty();
        let program = b"2 begincodespacerange <00> <7F> <8100> <81FF> endcodespacerange 1 begincidchar <41> 7 endcidchar";
        let encoding = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), program.to_vec()))));
        let font = composite_font(&mut doc, encoding, "41");
        let metrics = Metrics::from_dict(&doc, font.as_dict().unwrap());
        let old = metrics.encode("A").unwrap();
        assert_eq!(old, [0, 0x41]);
        assert_eq!(metrics.decode(&old), "A", "decoding omits the extra unmapped zero code");
        assert_eq!(metrics.encode_char('A'), None);
        assert!(TextFonts::new(&mut doc, "F", font, "A").err().unwrap().contains("mixed-length"));
    }

    #[test]
    fn commented_cmap_examples_do_not_change_emitted_bytes_or_cid_advances() {
        let mut doc = Document::new_empty();
        let encoding = br"/CIDInit /ProcSet findresource begin
12 dict begin begincmap
/CIDSystemInfo << /Registry (Example) /Ordering (Test) /Supplement 0 >> def
/CMapName /Example-H def /CMapType 1 def /WMode 0 def
% Documentation example: 1 begincidchar <41> 9 endcidchar
(Example: 1 begincidchar <41> 9 endcidchar) pop
/ExampleOperator /begincidchar def
1 begincodespacerange <00><7F> endcodespacerange
2 begincidchar <41>7 <42>9 endcidchar
endcmap CMapName currentdict /CMap defineresource pop end end";
        let encoding = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), encoding.to_vec()))));
        let Object::Dict(mut font) = composite_font(&mut doc, encoding, "41") else { panic!() };
        let unicode = br"/CIDInit /ProcSet findresource begin 12 dict begin begincmap
/CIDSystemInfo << /Registry (Example) /Ordering (Unicode) /Supplement 0 >> def
/CMapName /Example-UCS def /CMapType 2 def
1 begincodespacerange <00><7F> endcodespacerange
2 beginbfchar <41><0041><42><0042> endbfchar
% Documentation example: 2 beginbfchar <41> <0042> <42> <0041> endbfchar
(Example: 2 beginbfchar <41> <0042> <42> <0041> endbfchar) pop
/ExampleOperator /beginbfchar def
endcmap CMapName currentdict /CMap defineresource pop end end";
        font.set(b"ToUnicode".to_vec(), Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), unicode.to_vec())))));
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.encode_char('A'), Some(vec![0x41]));
        assert_eq!(metrics.encode_char('B'), Some(vec![0x42]));
        assert!((metrics.width(0x41) - 0.6).abs() < 1e-8);
        assert!((metrics.width(0x42) - 1.0).abs() < 1e-8);
        let fonts = TextFonts::new(&mut doc, "F", Object::Dict(font), "AB").unwrap();
        assert_eq!(fonts.show("AB", 12.0).unwrap(), b"/F 12.0000 Tf (AB) Tj\n");
        assert!((fonts.width("AB", 12.0) - 19.2).abs() < 1e-8);
    }

    #[test]
    fn wrapping_treats_crlf_as_one_break_and_preserves_empty_lines() {
        for (text, expected) in [
            ("", vec![""]),
            ("\r\n", vec!["", ""]),
            ("A\r\nB", vec!["A", "B"]),
            ("A\rB\nC", vec!["A", "B", "C"]),
            ("A\r\n\r\nB\r\n", vec!["A", "", "B", ""]),
            ("A\n\rB", vec!["A", "", "B"]),
        ] {
            let mut doc = Document::new_empty();
            let fonts = TextFonts::new(&mut doc, "F", standard_font("Helvetica"), text).unwrap();
            assert_eq!(fonts.wrap(text, 12.0, 200.0), expected, "generated {text:?}");
            assert_eq!(crate::wrap(text, 12.0, 200.0), expected, "standard {text:?}");
        }
    }
}
