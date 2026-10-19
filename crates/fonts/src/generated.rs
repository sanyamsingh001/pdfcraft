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
        if text.chars().take(65_537).count() > 65_536 {
            return Err("generated text exceeds 65536 characters; split it into smaller items".into());
        }
        let resolved = doc.resolve(&primary);
        let dict = resolved.as_dict().ok_or_else(|| "the selected text font is not a dictionary".to_string())?;
        let metrics = Metrics::from_dict(doc, dict);
        let mut missing = String::new();
        let mut primary_codes = HashMap::new();
        for ch in text.chars().filter(|c| !matches!(c, '\n' | '\r')) {
            if primary_codes.contains_key(&ch) || missing.contains(ch) {
                continue;
            }
            if let Some(bytes) = metrics.encode(&ch.to_string()).filter(|bytes| metrics.decode(bytes) == ch.to_string()) {
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
                while fonts.contains(fallback.name.as_bytes()) {
                    fallback.name.push('_');
                }
                fonts.set(fallback.name.as_bytes().to_vec(), Object::Ref(object));
            }
            fallback
        });
        Ok(Self { primary: name.into(), primary_codes, fallback, fonts })
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
        for para in text.split(['\n', '\r']) {
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
}
