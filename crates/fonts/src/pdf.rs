//! What content editing and redaction need from a PDF font: how to split a string into character
//! codes, each code's advance width, the glyph height (ascent, descent), and what each code means
//! (Unicode) — and back, which text the font can show. Widths come from the font dictionary
//! (`/Widths`, `/W`, `/DW`, `/MissingWidth`); the standard 14 fonts without widths use
//! exact advances for supported Latin faces, otherwise approximations. Meanings come from
//! `/ToUnicode`, else the encoding (`/Encoding` base and `/Differences` glyph names,
//! ISO 32000-2 Annex D).

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, Object};

/// How the bytes of a string map to codes.
#[derive(Clone, Debug, PartialEq)]
enum Codes {
    One,
    Two,
    /// Codespace ranges `(length, low, high)` from an embedded CMap.
    Ranges(Vec<(usize, Vec<u8>, Vec<u8>)>),
}

#[derive(Clone, Debug, PartialEq)]
enum Std14 {
    Helvetica,
    Times,
    Courier,
    Symbolic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    codes: Codes,
    /// Simple fonts: `/FirstChar` and `/Widths`.
    first: u32,
    widths: Vec<f64>,
    /// Composite fonts: CID → width, and CID ranges with one width.
    cid_widths: HashMap<u32, f64>,
    cid_ranges: Vec<(u32, u32, f64)>,
    /// Code → CID for embedded non-identity CMaps (`cidrange` / `cidchar`).
    cid_map: Vec<(u32, u32, u32)>,
    default: f64,
    std14: Option<Std14>,
    /// Code → selected encoding glyph, before ToUnicode changes extraction semantics.
    std14_glyphs: HashMap<u32, String>,
    /// Exact standard-face advances by code, using encoding geometry rather than ToUnicode.
    std14_advances: HashMap<u32, f64>,
    /// Glyph units → text space (0.001, or `/FontMatrix[0]` for Type 3).
    pub scale: f64,
    /// Glyph box in text space per unit of font size.
    pub ascent: f64,
    pub descent: f64,
    pub composite: bool,
    /// Code → Unicode text.
    unicode: HashMap<u32, String>,
    /// `/BaseFont`, and whether it is a subset (`ABCDEF+Name`: other glyphs are missing).
    pub base_font: String,
    pub subset: bool,
    /// Whether the PDF identifies this face as bold or italic. These come from the
    /// font name and, when present, `/FontDescriptor` flags/weight/angle.
    pub bold: bool,
    pub italic: bool,
    /// Bytes per code for writing new text (1 for simple fonts, else the codespace length).
    code_len: usize,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o))
        .and_then(|a| a.as_array().map(|a| a.iter().map(|x| doc.resolve(x).as_f64().unwrap_or(0.0)).collect()))
        .unwrap_or_default()
}

fn dict(doc: &Document, o: Option<&Object>) -> Option<Dict> {
    o.and_then(|o| doc.resolve(o).as_dict().cloned())
}

// Bounds allocation for destination strings as well as short character codes.
const MAX_CMAP_HEX_BYTES: usize = 65_536;

fn hex_bytes(tok: &str) -> Option<Vec<u8>> {
    let digits = tok.as_bytes().strip_prefix(b"<")?.strip_suffix(b">")?;
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    // Match COS hex-string semantics: PDF whitespace is ignored and an odd final
    // nibble is padded with zero. Bound decoded bytes; non-ASCII hex is invalid.
    let mut bytes = Vec::with_capacity((digits.len() / 2).min(MAX_CMAP_HEX_BYTES));
    let mut high = None;
    for byte in digits {
        if matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ') {
            continue;
        }
        let value = nibble(*byte)?;
        if bytes.len() >= MAX_CMAP_HEX_BYTES {
            return None;
        }
        match high.take() {
            None => high = Some(value),
            Some(high) => bytes.push(high << 4 | value),
        }
    }
    if let Some(high) = high {
        bytes.push(high << 4);
    }
    (!bytes.is_empty()).then_some(bytes)
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| (a << 8) | u32::from(*x))
}

fn style_from_name(name: &str) -> (bool, bool) {
    let name = name.to_ascii_lowercase();
    let name = name.split_once('+').map_or(name.as_str(), |(_, n)| n);
    (
        ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
    )
}

/// Lexical CMap operands/operators. Comments are whitespace; literal strings, names and
/// dictionaries are opaque operands, never executable mapping operators. Keep array brackets
/// for bfrange destinations and PDF-delimited adjacent hex strings as separate operands.
fn cmap_tokens(data: &[u8]) -> Vec<&str> {
    let mut lexer = pdfcraft_cos::Lexer::new(data, 0);
    let mut tokens = Vec::new();
    loop {
        lexer.skip_ws();
        let start = lexer.pos;
        let Some(first) = data.get(start) else { break };
        match first {
            b'[' | b']' => {
                lexer.pos += 1;
                tokens.push(if *first == b'[' { "[" } else { "]" });
            }
            b'/' | b'(' | b'<' => {
                if lexer.object().is_err() {
                    return Vec::new();
                }
                if *first == b'<' && data.get(start + 1) != Some(&b'<') {
                    tokens.push(std::str::from_utf8(&data[start..lexer.pos]).unwrap_or(""));
                } else {
                    // Keep an opaque operand's position so skipping it cannot join mappings.
                    tokens.push("");
                }
            }
            _ => {
                let token = lexer.token();
                if token.is_empty() {
                    return Vec::new();
                }
                tokens.push(std::str::from_utf8(token).unwrap_or(""));
            }
        }
    }
    tokens
}

/// Codespace ranges and CID mappings from an embedded CMap stream.
#[allow(clippy::type_complexity)]
fn parse_cmap(data: &[u8]) -> (Vec<(usize, Vec<u8>, Vec<u8>)>, Vec<(u32, u32, u32)>) {
    let toks = cmap_tokens(data);
    let (mut spaces, mut cids) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "begincodespacerange" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endcodespacerange" {
                    if let (Some(lo), Some(hi)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1]))
                        && lo.len() == hi.len()
                    {
                        spaces.push((lo.len(), lo, hi));
                    }
                    i += 2;
                }
            }
            "begincidrange" => {
                i += 1;
                while i + 2 < toks.len() && toks[i] != "endcidrange" {
                    if let (Some(lo), Some(hi), Ok(c)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1]), toks[i + 2].parse::<u32>()) {
                        cids.push((be(&lo), be(&hi), c));
                    }
                    i += 3;
                }
            }
            "begincidchar" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endcidchar" {
                    if let (Some(code), Ok(c)) = (hex_bytes(toks[i]), toks[i + 1].parse::<u32>()) {
                        cids.push((be(&code), be(&code), c));
                    }
                    i += 2;
                }
            }
            _ => {}
        }
        i += 1;
    }
    spaces.sort_by_key(|s| s.0);
    (spaces, cids)
}

/// Validate the horizontal, fixed-width encodings generated text can safely lay out.
/// Unknown predefined maps and inherited, vertical or mixed-length maps need another writer.
pub fn validate_generated_encoding(doc: &Document, font: &Dict) -> Result<(), String> {
    if font.name(b"Subtype") != Some(b"Type0") {
        return Ok(());
    }
    let unsupported = || {
        "this composite font encoding is not supported for generated text; choose Identity-H or a fixed-length horizontal CMap with explicit CID mappings".to_string()
    };
    let vertical = || "vertical composite fonts are not supported for generated text; choose a horizontal font".to_string();
    let encoding = font.get(b"Encoding").map(|o| doc.resolve(o)).ok_or_else(unsupported)?;
    match &*encoding {
        Object::Name(name) if name == b"Identity-H" => return Ok(()),
        Object::Name(name) if name.ends_with(b"-V") => return Err(vertical()),
        Object::Stream(stream) => {
            if stream.dict.contains(b"UseCMap") {
                return Err(unsupported());
            }
            if let Some(mode) = stream.dict.get(b"WMode") {
                match doc.resolve(mode).as_int() {
                    Some(0) => {}
                    Some(1) => return Err(vertical()),
                    _ => return Err(unsupported()),
                }
            }
            let data = stream.decoded().map_err(|_| unsupported())?;
            // Tokenize names/strings/comments with the PDF lexer; text searches could miss
            // escaped names or mistake a comment/string for a writing-mode declaration.
            let mut lexer = pdfcraft_cos::Lexer::new(&data, 0);
            loop {
                lexer.skip_ws();
                let Some(first) = data.get(lexer.pos) else { break };
                if first.is_ascii_alphabetic() {
                    if lexer.token() == b"usecmap" {
                        return Err(unsupported());
                    }
                    continue;
                }
                match lexer.object().map_err(|_| unsupported())? {
                    Object::Name(name) if name == b"WMode" => {
                        match lexer.object().map_err(|_| unsupported())?.as_int() {
                            Some(0) => {}
                            Some(1) => return Err(vertical()),
                            _ => return Err(unsupported()),
                        }
                        if !lexer.eat_keyword(b"def") {
                            return Err(unsupported());
                        }
                    }
                    Object::Name(name) if name == b"UseCMap" => return Err(unsupported()),
                    _ => {}
                }
            }
            let (spaces, cids) = parse_cmap(&data);
            let Some((length, _, _)) = spaces.first() else { return Err(unsupported()) };
            if !(1..=4).contains(length) || spaces.iter().any(|(len, low, high)| len != length || low > high) {
                return Err(
                    "mixed-length or invalid CMap codespaces are not supported for generated text; choose a fixed-length horizontal font".into()
                );
            }
            if cids.is_empty() || cids.iter().any(|(low, high, cid)| high.checked_sub(*low).and_then(|n| cid.checked_add(n)).is_none()) {
                return Err(unsupported());
            }
            Ok(())
        }
        _ => Err(unsupported()),
    }
}

impl Metrics {
    /// Metrics for text without a usable font (Helvetica-like).
    pub fn fallback() -> Self {
        Metrics {
            codes: Codes::One,
            first: 0,
            widths: Vec::new(),
            cid_widths: HashMap::new(),
            cid_ranges: Vec::new(),
            cid_map: Vec::new(),
            default: 500.0,
            std14: Some(Std14::Helvetica),
            std14_glyphs: (32..127u8).map(|c| (u32::from(c), char::from(c).to_string())).collect(),
            std14_advances: HashMap::new(),
            scale: 0.001,
            ascent: 0.9,
            descent: -0.25,
            composite: false,
            unicode: (32..127u8).map(|c| (u32::from(c), char::from(c).to_string())).collect(),
            base_font: "Helvetica".into(),
            subset: false,
            bold: false,
            italic: false,
            code_len: 1,
        }
    }

    pub fn from_dict(doc: &Document, font: &Dict) -> Self {
        let mut m = Self::read_metrics(doc, font);
        let base = if let Some(name) = font.name(b"BaseFont") {
            String::from_utf8_lossy(name).into_owned()
        } else if font.name(b"Subtype") == Some(b"Type3") {
            // Type3 has no BaseFont. Read an explicit descriptor identity, never guess a
            // family from the page's resource alias or from the font's deprecated Name.
            dict(doc, font.get(b"FontDescriptor"))
                .and_then(|d| d.name(b"FontName").map(|n| String::from_utf8_lossy(n).into_owned()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        m.subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
        let (bold, italic) = style_from_name(&base);
        m.bold |= bold;
        m.italic |= italic;
        m.base_font = base;
        m.code_len = match &m.codes {
            Codes::One => 1,
            Codes::Two => 2,
            Codes::Ranges(r) => r.last().map_or(2, |x| x.0),
        };
        m.unicode = if m.composite { HashMap::new() } else { encoding_map(doc, font) };
        if m.std14.is_some() {
            m.std14_glyphs = m.unicode.clone();
            m.std14_advances = standard_advances(doc, font);
        }
        if let Some(Object::Stream(s)) = font.get(b"ToUnicode").map(|t| doc.resolve(t)).as_deref()
            && let Ok(data) = s.decoded()
        {
            parse_to_unicode(&data, &mut m.unicode);
        }
        m
    }

    fn read_metrics(doc: &Document, font: &Dict) -> Self {
        let mut m = Metrics::fallback();
        m.std14 = None;
        let subtype = font.name(b"Subtype").unwrap_or(b"Type1").to_vec();
        let descriptor;
        if subtype == b"Type0" {
            m.composite = true;
            m.default = 1000.0;
            m.codes = Codes::Two;
            if let Some(e) = font.get(b"Encoding").map(|e| doc.resolve(e))
                && let Object::Stream(s) = &*e
                && let Ok(data) = s.decoded()
            {
                let (spaces, cids) = parse_cmap(&data);
                if !spaces.is_empty() {
                    m.codes = Codes::Ranges(spaces);
                }
                m.cid_map = cids;
            }
            let desc = font
                .get(b"DescendantFonts")
                .map(|d| doc.resolve(d))
                .and_then(|a| a.as_array().and_then(|a| a.first().cloned()))
                .and_then(|d| doc.resolve(&d).as_dict().cloned())
                .unwrap_or_default();
            if let Some(dw) = desc.get(b"DW").and_then(|d| doc.resolve(d).as_f64()) {
                m.default = dw;
            }
            if let Some(w) = desc.get(b"W").map(|w| doc.resolve(w)).and_then(|w| w.as_array().cloned()) {
                let mut i = 0;
                while i < w.len() {
                    let Some(c0) = doc.resolve(&w[i]).as_f64().map(|v| v as u32) else { break };
                    match w.get(i + 1).map(|o| doc.resolve(o)) {
                        Some(a) if a.as_array().is_some() => {
                            for (k, x) in a.as_array().into_iter().flatten().enumerate() {
                                if let Some(v) = doc.resolve(x).as_f64() {
                                    m.cid_widths.insert(c0 + k as u32, v);
                                }
                            }
                            i += 2;
                        }
                        Some(c1) => {
                            if let (Some(c1), Some(v)) = (c1.as_f64(), w.get(i + 2).and_then(|x| doc.resolve(x).as_f64())) {
                                m.cid_ranges.push((c0, c1 as u32, v));
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
            descriptor = dict(doc, desc.get(b"FontDescriptor"));
        } else {
            m.first = font.get(b"FirstChar").and_then(|f| doc.resolve(f).as_f64()).unwrap_or(0.0).max(0.0) as u32;
            m.widths = nums(doc, font.get(b"Widths"));
            descriptor = dict(doc, font.get(b"FontDescriptor"));
            m.default = descriptor.as_ref().and_then(|d| d.get(b"MissingWidth")).and_then(|w| doc.resolve(w).as_f64()).unwrap_or(0.0);
            if subtype == b"Type3" {
                let fm = nums(doc, font.get(b"FontMatrix"));
                if fm.len() == 6 {
                    m.scale = fm[0];
                    let bbox = nums(doc, font.get(b"FontBBox"));
                    if bbox.len() == 4 && fm[3] != 0.0 {
                        m.ascent = (bbox[3] * fm[3]).max(0.5);
                        m.descent = (bbox[1] * fm[3]).min(-0.1);
                    }
                }
            } else if m.widths.is_empty() {
                let base = String::from_utf8_lossy(font.name(b"BaseFont").unwrap_or(b"")).to_ascii_lowercase();
                m.std14 = Some(if base.contains("courier") {
                    Std14::Courier
                } else if base.contains("times") {
                    Std14::Times
                } else if base.contains("symbol") || base.contains("dingbats") {
                    Std14::Symbolic
                } else {
                    Std14::Helvetica
                });
            }
        }
        if let Some(d) = descriptor {
            let flags = d.get(b"Flags").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0).max(0.0) as u32;
            let angle = d.get(b"ItalicAngle").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0);
            let weight = d.get(b"FontWeight").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(0.0);
            m.italic |= flags & 64 != 0 || angle.abs() > 0.1;
            m.bold |= flags & 262_144 != 0 || weight >= 600.0;
            // Ascent/Descent are optional for Type3. An identity-only descriptor must not
            // discard the metrics already derived from its FontBBox/FontMatrix.
            if let Some(a) = d.get(b"Ascent").and_then(|v| doc.resolve(v).as_f64()) {
                let a = a / 1000.0;
                m.ascent = if a > 0.3 { a.min(1.5) } else { 0.9 };
            }
            if let Some(de) = d.get(b"Descent").and_then(|v| doc.resolve(v).as_f64()) {
                let de = de / 1000.0;
                m.descent = if de < -0.05 { de.max(-0.6) } else { -0.25 };
            }
        }
        m
    }

    /// Split a string into `(code, byte length)`.
    pub fn codes(&self, s: &[u8]) -> Vec<(u32, usize)> {
        let mut out = Vec::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            let len = match &self.codes {
                Codes::One => 1,
                Codes::Two => 2,
                Codes::Ranges(r) => r
                    .iter()
                    .find(|(len, lo, hi)| s.get(i..i + len).is_some_and(|b| b.iter().zip(lo.iter().zip(hi)).all(|(x, (l, h))| x >= l && x <= h)))
                    .map_or(r.first().map_or(1, |x| x.0), |x| x.0),
            };
            let len = len.min(s.len() - i).max(1);
            out.push((be(&s[i..i + len]), len));
            i += len;
        }
        out
    }

    fn cid(&self, code: u32) -> u32 {
        if self.cid_map.is_empty() {
            return code;
        }
        self.cid_map.iter().find(|(lo, hi, _)| code >= *lo && code <= *hi).map_or(0, |(lo, _, c)| c + (code - lo))
    }

    /// The advance of `code` in text space per unit of font size.
    pub fn width(&self, code: u32) -> f64 {
        if self.composite {
            let cid = self.cid(code);
            let w = self.cid_widths.get(&cid).copied().or_else(|| self.cid_ranges.iter().find(|(a, b, _)| cid >= *a && cid <= *b).map(|r| r.2));
            return w.unwrap_or(self.default) * self.scale;
        }
        if let Some(w) = code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            return w * self.scale;
        }
        if let Some(width) = self.std14_advances.get(&code) {
            return *width;
        }
        match &self.std14 {
            Some(Std14::Courier) => 0.6,
            Some(Std14::Symbolic) => 0.75,
            Some(f) => {
                let glyph = self.std14_glyphs.get(&code).map_or("n", String::as_str);
                let w = crate::helvetica_width(glyph, 1.0);
                if *f == Std14::Times { w * 0.92 } else { w }
            }
            None => self.default * self.scale,
        }
    }

    /// Word spacing applies to the single-byte code 32 (§9.3.3).
    pub fn is_space(&self, code: u32, len: usize) -> bool {
        code == 32 && len == 1
    }
}

impl Metrics {
    /// The text a string shows (codes without a known meaning are left out).
    pub fn decode(&self, s: &[u8]) -> String {
        self.codes(s).into_iter().filter_map(|(c, _)| self.unicode.get(&c).cloned()).collect()
    }

    /// The Unicode text of one code, if known.
    pub fn text_of(&self, code: u32) -> Option<&str> {
        self.unicode.get(&code).map(String::as_str)
    }

    /// Whether the font has a glyph for `code` (subset fonts lack the glyphs they don't use).
    fn has_glyph(&self, code: u32) -> bool {
        if self.composite {
            let cid = self.cid(code);
            return self.cid_widths.contains_key(&cid) || self.cid_ranges.iter().any(|(a, b, _)| cid >= *a && cid <= *b) || !self.subset;
        }
        match code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            Some(w) => *w > 0.0 || code == 32,
            None => self.widths.is_empty() && !self.subset,
        }
    }

    /// Encode one scalar only if the emitted bytes are exactly one mapped character code.
    /// A Unicode round trip alone is insufficient: decoding deliberately omits unknown codes.
    pub fn encode_char(&self, ch: char) -> Option<Vec<u8>> {
        let text = ch.to_string();
        let bytes = self.encode(&text)?;
        let codes = self.codes(&bytes);
        let [(code, length)] = codes.as_slice() else { return None };
        if *length != bytes.len() || self.text_of(*code) != Some(text.as_str()) {
            return None;
        }
        if let Codes::Ranges(ranges) = &self.codes {
            if !ranges
                .iter()
                .any(|(length, low, high)| *length == bytes.len() && low.as_slice() <= bytes.as_slice() && bytes.as_slice() <= high.as_slice())
            {
                return None;
            }
            if !self.cid_map.iter().any(|(low, high, _)| *low <= *code && *code <= *high) {
                return None;
            }
        }
        Some(bytes)
    }

    /// The bytes that show `text` in this font, or `None` if some character has no code or
    /// no glyph in it (the caller then substitutes another font).
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        let mut reverse: HashMap<&str, u32> = HashMap::new();
        for (code, t) in &self.unicode {
            if self.has_glyph(*code) {
                reverse.entry(t.as_str()).and_modify(|c| *c = (*c).min(*code)).or_insert(*code);
            }
        }
        let mut out = Vec::with_capacity(text.len() * self.code_len);
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            let code = *reverse.get(ch.encode_utf8(&mut buf) as &str)?;
            let bytes = code.to_be_bytes();
            out.extend_from_slice(&bytes[4 - self.code_len.clamp(1, 4)..]);
        }
        Some(out)
    }
}

/// UTF-16BE (with surrogates) from CMap hex bytes.
fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
    String::from_utf16_lossy(&units)
}

/// `bfchar`/`bfrange` entries of a ToUnicode CMap.
fn parse_to_unicode(data: &[u8], out: &mut HashMap<u32, String>) {
    let toks = cmap_tokens(data);
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "beginbfchar" => {
                i += 1;
                while i + 1 < toks.len() && toks[i] != "endbfchar" {
                    if let (Some(c), Some(u)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1])) {
                        out.insert(be(&c), utf16(&u));
                    }
                    i += 2;
                }
            }
            "beginbfrange" => {
                i += 1;
                while i + 2 < toks.len() && toks[i] != "endbfrange" {
                    let (Some(lo), Some(hi)) = (hex_bytes(toks[i]), hex_bytes(toks[i + 1])) else { break };
                    let (lo, hi) = (be(&lo), be(&hi));
                    if toks[i + 2] == "[" {
                        let mut j = i + 3;
                        let mut code = (lo <= hi).then_some(lo);
                        while j < toks.len() && toks[j] != "]" {
                            if let Some(current) = code {
                                if let Some(u) = hex_bytes(toks[j]) {
                                    out.insert(current, utf16(&u));
                                }
                                code = current.checked_add(1).filter(|next| *next <= hi);
                            }
                            // Ignore excess destinations, but consume the complete array.
                            j += 1;
                        }
                        i = j + 1;
                    } else {
                        if let Some(u) = hex_bytes(toks[i + 2])
                            && hi >= lo
                            && hi - lo < 65536
                        {
                            let mut units: Vec<u16> = u.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                            for code in lo..=hi {
                                out.insert(code, String::from_utf16_lossy(&units));
                                if let Some(last) = units.last_mut() {
                                    *last = last.wrapping_add(1);
                                }
                            }
                        }
                        i += 3;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// A glyph name's Unicode (Annex D names, `uniXXXX`, `uXXXX[XX]`, single letters).
pub fn glyph_unicode(name: &str) -> Option<char> {
    if let Ok(i) = crate::encodings::NAMES.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return char::from_u32(crate::encodings::NAMES[i].1);
    }
    let hex = name.strip_prefix("uni").filter(|h| h.len() == 4).or_else(|| name.strip_prefix('u').filter(|h| (4..=6).contains(&h.len())))?;
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Standard-face widths use the selected glyph names, including Differences whose names
/// have no Unicode entry. ToUnicode is an extraction map and never selects a glyph's width.
fn standard_advances(doc: &Document, font: &Dict) -> HashMap<u32, f64> {
    let base = font.name(b"BaseFont").unwrap_or_default();
    if crate::standard_metrics::width(base, b"A").is_none() {
        return HashMap::new();
    }
    let encoding = font.get(b"Encoding").map(|o| doc.resolve(o));
    let base_encoding = match encoding.as_deref() {
        Some(Object::Name(name)) => name.as_slice(),
        Some(Object::Dict(d)) => d.name(b"BaseEncoding").unwrap_or_default(),
        _ => b"",
    };
    let mut widths: HashMap<u32, f64> = crate::standard_metrics::base_glyphs(base_encoding)
        .iter()
        .enumerate()
        .filter_map(|(code, glyph)| crate::standard_metrics::width(base, glyph.as_bytes()).map(|width| (code as u32, width)))
        .collect();
    if let Some(Object::Dict(d)) = encoding.as_deref()
        && let Some(differences) = d.get(b"Differences").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned())
    {
        let mut code = None;
        for item in differences {
            match item {
                Object::Int(n) => code = u8::try_from(n).ok(),
                Object::Name(name) => {
                    if let Some(current) = code {
                        match crate::standard_metrics::width(base, &name) {
                            Some(width) => widths.insert(u32::from(current), width),
                            None => widths.remove(&u32::from(current)),
                        };
                        code = current.checked_add(1);
                    }
                }
                _ => {}
            }
        }
    }
    widths
}

/// Code → glyph Unicode from a simple font encoding, independent of extraction semantics.
fn encoding_map(doc: &Document, font: &Dict) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let enc = font.get(b"Encoding").map(|e| doc.resolve(e));
    let base_name = match enc.as_deref() {
        Some(Object::Name(n)) => Some(n.clone()),
        Some(Object::Dict(d)) => d.name(b"BaseEncoding").map(<[u8]>::to_vec),
        _ => None,
    };
    let table: &[u32; 256] = match base_name.as_deref() {
        Some(b"WinAnsiEncoding") => &crate::encodings::WIN_ANSI,
        Some(b"MacRomanEncoding") => &crate::encodings::MAC_ROMAN,
        _ => &crate::encodings::STANDARD,
    };
    for (code, u) in table.iter().enumerate() {
        if let Some(c) = char::from_u32(*u).filter(|_| *u != 0) {
            out.insert(code as u32, c.to_string());
        }
    }
    if let Some(Object::Dict(d)) = enc.as_deref()
        && let Some(diffs) = d.get(b"Differences").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned())
    {
        // Simple-font codes are bytes. An invalid reset or an exhausted run must not
        // wrap into another code; skip names until the next valid integer reset.
        let mut code = None;
        for item in diffs {
            match &item {
                Object::Int(n) => code = u8::try_from(*n).ok(),
                Object::Name(n) => {
                    if let Some(current) = code {
                        match glyph_unicode(&String::from_utf8_lossy(n)) {
                            Some(c) => out.insert(u32::from(current), c.to_string()),
                            None => out.remove(&u32::from(current)),
                        };
                        code = current.checked_add(1);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::*;

    fn font(doc: &mut Document, entries: Vec<(&str, Object)>) -> Dict {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Font"));
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v);
        }
        let _ = doc;
        d
    }

    #[test]
    fn cmap_hex_tokens_are_ascii_delimited_and_bounded() {
        assert_eq!(hex_bytes("<aBcD00>"), Some(vec![0xab, 0xcd, 0]));
        for token in ["<a B\tc\nD\r0\x0c0>", "<aBcD00\0>"] {
            assert_eq!(hex_bytes(token), Some(vec![0xab, 0xcd, 0]));
        }
        assert_eq!(hex_bytes("<000>"), Some(vec![0, 0]));
        assert_eq!(hex_bytes("<a B c>"), Some(vec![0xab, 0xc0]));
        for malformed in ["<0\u{e9}0>", "<0\u{1f600}0>", "<GG>", "<>", "< \t\r\n>", "00", "<<00>>"] {
            assert_eq!(hex_bytes(malformed), None, "{malformed:?}");
        }
        assert_eq!(hex_bytes(&format!("<{}>", "00".repeat(MAX_CMAP_HEX_BYTES))).unwrap().len(), MAX_CMAP_HEX_BYTES);
        assert!(hex_bytes(&format!("<{}>", "00".repeat(MAX_CMAP_HEX_BYTES + 1))).is_none());
        assert_eq!(hex_bytes(&format!("<{}>", "0 0 ".repeat(MAX_CMAP_HEX_BYTES))).unwrap().len(), MAX_CMAP_HEX_BYTES);
        assert!(hex_bytes(&format!("<{}0>", "00".repeat(MAX_CMAP_HEX_BYTES))).is_none());
    }

    #[test]
    fn cmap_hex_spacing_and_odd_nibbles_keep_mappings_codes_and_cid_advances() {
        use crate::generated::TextFonts;

        for (space, source, destination, text, byte) in [("<0 0> <7 F>", "<4 1>", "<00 41>", "A", 0x41), ("<0> <7F>", "<4>", "<004>", "@", 0x40)] {
            let mut doc = Document::new_empty();
            let program = format!("1 begincodespacerange {space} endcodespacerange 1 begincidchar {source} 7 endcidchar");
            let encoding = doc.add(Object::Stream(Stream::from_raw(Dict::new(), program.into_bytes())));
            let unicode = format!("1 beginbfchar {source} {destination} endbfchar");
            let unicode = doc.add(Object::Stream(Stream::from_raw(Dict::new(), unicode.into_bytes())));
            let mut descendant = Dict::new();
            descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
            descendant.set(b"DW".to_vec(), Object::Int(1000));
            descendant.set(b"W".to_vec(), Object::Array(vec![Object::Int(7), Object::Array(vec![Object::Int(600)])]));
            let font = font(
                &mut doc,
                vec![
                    ("Subtype", Object::name("Type0")),
                    ("Encoding", Object::Ref(encoding)),
                    ("ToUnicode", Object::Ref(unicode)),
                    ("DescendantFonts", Object::Array(vec![Object::Dict(descendant)])),
                ],
            );
            validate_generated_encoding(&doc, &font).unwrap();
            let metrics = Metrics::from_dict(&doc, &font);
            assert_eq!(metrics.decode(&[byte]), text);
            assert_eq!(metrics.encode_char(text.chars().next().unwrap()), Some(vec![byte]));
            assert!((metrics.width(u32::from(byte)) - 0.6).abs() < 1e-8);
            let fonts = TextFonts::new(&mut doc, "F", Object::Dict(font), text).unwrap();
            assert_eq!(fonts.show(text, 12.0).unwrap(), format!("/F 12.0000 Tf ({text}) Tj\n").as_bytes());
            assert!((fonts.width(text, 12.0) - 7.2).abs() < 1e-8);
        }
    }

    #[test]
    fn to_unicode_array_ranges_stop_at_the_declared_high_and_resume_after_the_bracket() {
        let mut out = HashMap::new();
        parse_to_unicode(
            b"2 beginbfrange <0001> <0002> [<0041> <0042> <0043>] <0010> <0010> [<0044>] endbfrange \
              1 beginbfchar <0020> <0045> endbfchar",
            &mut out,
        );
        assert_eq!(out.len(), 4);
        assert_eq!(out.get(&1).map(String::as_str), Some("A"));
        assert_eq!(out.get(&2).map(String::as_str), Some("B"));
        assert!(!out.contains_key(&3), "an extra destination must not spill beyond the range");
        assert_eq!(out.get(&0x10).map(String::as_str), Some("D"));
        assert_eq!(out.get(&0x20).map(String::as_str), Some("E"));
    }

    #[test]
    fn to_unicode_array_ranges_allow_short_arrays_and_skip_reversed_ranges() {
        let mut out = HashMap::new();
        parse_to_unicode(b"2 beginbfrange <0005> <0004> [<0041> <0042>] <0010> <0012> [<0044>] endbfrange", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get(&0x10).map(String::as_str), Some("D"));
        assert!(!out.contains_key(&0x11));
        assert!(!out.contains_key(&0x12));
    }

    #[test]
    fn to_unicode_array_ranges_include_the_last_u32_code() {
        let mut out = HashMap::new();
        // Two valid boundary entries and a following single-entry range; no large expansion.
        parse_to_unicode(b"2 beginbfrange <FFFFFFFE> <FFFFFFFF> [<0041> <0042>] <0000> <0000> [<0043>] endbfrange", &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out.get(&(u32::MAX - 1)).map(String::as_str), Some("A"));
        assert_eq!(out.get(&u32::MAX).map(String::as_str), Some("B"));
        assert_eq!(out.get(&0).map(String::as_str), Some("C"));
    }

    #[test]
    fn simple_fonts_decode_and_encode_through_their_encoding() {
        let mut doc = Document::new_empty();
        let f = font(
            &mut doc,
            vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica")), ("Encoding", Object::name("WinAnsiEncoding"))],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(b"Caf\xe9 \x80 \x93x\x94"), "Café € “x”");
        assert_eq!(m.encode("Café €").as_deref(), Some(&b"Caf\xe9 \x80"[..]));
        assert_eq!(m.encode("Ω"), None, "not in WinAnsi");
        // Differences rename codes by glyph name.
        let mut enc = Dict::new();
        enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("eacute"), Object::name("uni03A9")]));
        let f = font(&mut doc, vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Custom")), ("Encoding", Object::Dict(enc))]);
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(b"AB"), "éΩ");
        assert_eq!(m.encode("Ω").as_deref(), Some(&b"B"[..]));
    }

    #[test]
    fn differences_codes_are_bytes_and_invalid_runs_resume_only_at_valid_resets() {
        let doc = Document::new_empty();
        let Object::Dict(mut font) = crate::generated::standard_font("Helvetica") else { panic!() };
        let ordinary = Metrics::from_dict(&doc, &font);
        // Original dictionary-only boundary cases; no external font or PDF fixture.
        for invalid in [-1, 256, i64::MIN, i64::MAX, i64::from(u32::MAX), (1i64 << 32) + 73] {
            let mut encoding = Dict::new();
            encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
            encoding.set(
                b"Differences".to_vec(),
                Object::Array(vec![
                    Object::name("W"),
                    Object::Int(invalid),
                    Object::name("W"),
                    Object::name("I"),
                    Object::Int(65),
                    Object::name("Z"),
                ]),
            );
            font.set(b"Encoding".to_vec(), Object::Dict(encoding));
            let metrics = Metrics::from_dict(&doc, &font);
            for code in 0..=255u8 {
                let expected = if code == 65 { "Z".into() } else { ordinary.decode(&[code]) };
                assert_eq!(metrics.decode(&[code]), expected, "reset={invalid}, code={code}");
            }
        }
        let mut encoding = Dict::new();
        encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
        encoding.set(
            b"Differences".to_vec(),
            Object::Array(vec![
                Object::name("W"), // No initial reset: ignore.
                Object::Int(255),
                Object::name("W"),
                Object::name("I"), // End of the byte range.
                Object::Int(0),
                Object::name("I"), // A later valid reset still applies.
            ]),
        );
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.decode(&[255, 0]), "WI");
        assert!((metrics.width(255) - 0.944).abs() < 1e-8);
        assert!((metrics.width(0) - 0.278).abs() < 1e-8);
        assert_eq!(metrics.decode(&[1]), ordinary.decode(&[1]));
        assert_eq!(metrics.unicode.keys().max(), Some(&255), "an exhausted run must not allocate a code above 255");
    }

    #[test]
    fn subset_fonts_only_encode_the_glyphs_they_have() {
        let mut doc = Document::new_empty();
        // Widths for a, b only (97, 98); c (99) is 0.
        let widths = Object::Array(vec![Object::Int(500), Object::Int(520), Object::Int(0)]);
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("TrueType")),
                ("BaseFont", Object::name("ABCDEF+Arial")),
                ("FirstChar", Object::Int(97)),
                ("Widths", widths),
                ("Encoding", Object::name("WinAnsiEncoding")),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert!(m.subset);
        assert_eq!(m.encode("ab").as_deref(), Some(&b"ab"[..]));
        assert_eq!(m.encode("abc"), None);
    }

    #[test]
    fn composite_fonts_use_to_unicode() {
        let mut doc = Document::new_empty();
        let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
            2 beginbfchar <0003> <0020> <0010> <00E9> endbfchar 1 beginbfrange <0024> <0026> <0041> endbfrange \
            1 beginbfrange <0030> <0031> [<0048> <0069>] endbfrange end";
        let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
        let mut desc = Dict::new();
        desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        desc.set(
            b"W".to_vec(),
            Object::Array(vec![
                Object::Int(3),
                Object::Array(vec![Object::Int(250)]),
                Object::Int(16),
                Object::Int(16),
                Object::Int(500),
                Object::Int(36),
                Object::Int(49),
                Object::Int(600),
            ]),
        );
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("BaseFont", Object::name("QWERTY+Noto")),
                ("Encoding", Object::name("Identity-H")),
                ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
                ("ToUnicode", Object::Ref(tu)),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(&[0, 0x24, 0, 0x25, 0, 3, 0, 0x30, 0, 0x31, 0, 0x10]), "AB Hié");
        assert_eq!(m.encode("Hi A").as_deref(), Some(&[0, 0x30, 0, 0x31, 0, 3, 0, 0x24][..]));
        assert_eq!(m.encode("Z"), None);
    }

    #[test]
    fn glyph_names() {
        assert_eq!(glyph_unicode("quotedblleft"), Some('“'));
        assert_eq!(glyph_unicode("uni20AC"), Some('€'));
        assert_eq!(glyph_unicode("u1F600"), Some('😀'));
        assert_eq!(glyph_unicode("nonsense"), None);
    }

    #[test]
    fn type3_identity_uses_explicit_descriptor_without_guessing_or_changing_geometry() {
        let mut doc = Document::new_empty();
        let mut font = Dict::new();
        font.set(b"Subtype".to_vec(), Object::name("Type3"));
        font.set(b"Name".to_vec(), Object::name("PCJapanese"));
        font.set(b"FontBBox".to_vec(), Object::Array([-100, -400, 1000, 1200].map(Object::Int).to_vec()));
        font.set(b"FontMatrix".to_vec(), Object::Array([0.001, 0.0, 0.0, 0.001, 0.0, 0.0].map(Object::Real).to_vec()));
        let before = Metrics::from_dict(&doc, &font);
        assert_eq!(before.base_font, "", "resource-style names do not identify an unknown imported face");
        let mut descriptor = Dict::new();
        descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        descriptor.set(b"FontName".to_vec(), Object::name("Original-Outlines"));
        descriptor.set(b"Flags".to_vec(), Object::Int(4));
        descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
        font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
        let after = Metrics::from_dict(&doc, &font);
        assert_eq!(after.base_font, "Original-Outlines");
        assert_eq!((after.ascent, after.descent, after.scale), (before.ascent, before.descent, before.scale));
        assert_eq!((after.ascent, after.descent), (1.2, -0.4));
        font.set(b"BaseFont".to_vec(), Object::name("AuthorFont-Bold"));
        assert_eq!(Metrics::from_dict(&doc, &font).base_font, "AuthorFont-Bold", "an explicit existing BaseFont retains precedence");
        font.remove(b"BaseFont");
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        assert_eq!(Metrics::from_dict(&doc, &font).base_font, "", "other imported font kinds retain their existing identity semantics");
    }
}
