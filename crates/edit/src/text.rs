//! Edit a PDF ▸ Edit text: the text lines already on a page, and replacing a line's text in place.
//!
//! A *line* is a run of text-showing operators inside one `BT … ET` on the same baseline, read
//! across the page's content streams. Replacing it patches only affected pieces (as new objects,
//! since streams may be shared): the first operator of the line shows the new text, the others are
//! dropped, and every positioning operator stays, so the lines after it keep their places
//! (`Td`, `T*` and friends move from the line matrix, which showing text doesn't change).
//!
//! The new text uses the line's own font when every character has a code and a glyph in it;
//! otherwise it is set in Helvetica (WinAnsi), and the result says the font was substituted.
//! Reflowing a paragraph to a new width is not done here (see `edit.text-reflow`).

use std::collections::HashMap;
use std::rc::Rc;

use crate::content::Content;
use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, Object, PdfString};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{Type3Fallback, type3_encode};

use crate::EditError;

/// One line of existing text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// The line's text as shown.
    pub text: String,
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The font's resource name and `/BaseFont`, and its size in text space.
    pub font: String,
    // Resource identity is raw PDF Name bytes; the public string is a display label only.
    font_name: Vec<u8>,
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The current fill colour as RGB.
    pub color: [f64; 3],
    /// Whether new text in this font can only be shown by substituting another font (no
    /// Unicode mapping for the line, so nothing could be reused).
    pub decodable: bool,
    stream: usize,
    /// Identity of the innermost marked-content sequence, including its parent scope.
    scope: Option<usize>,
    ops: Vec<usize>,
    /// Where the line starts: text matrix (text space), the state there, and its `BT`.
    origin: Origin,
}

/// Text and graphics state captured for rewriting.
#[derive(Clone, Debug, PartialEq)]
struct TextState {
    font: Option<(Vec<u8>, f64)>,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    fill: Vec<Op>,
}

impl TextState {
    fn of(ts: &Ts) -> Self {
        TextState {
            font: ts.font.as_ref().map(|(n, _)| (n.clone(), ts.size)),
            char_spacing: ts.char_spacing,
            word_spacing: ts.word_spacing,
            scale: ts.scale,
            leading: ts.leading,
            rise: ts.rise,
            fill: ts.fill.clone(),
        }
    }

    /// Operators that set this state (inside a text object).
    fn ops(&self) -> Vec<Op> {
        let n = pdfcraft_content::num;
        let mut v = Vec::new();
        if let Some((f, size)) = &self.font {
            v.push(Op::new("Tf", vec![Object::Name(f.clone()), n(*size)]));
        }
        v.push(Op::new("Tc", vec![n(self.char_spacing)]));
        v.push(Op::new("Tw", vec![n(self.word_spacing)]));
        v.push(Op::new("Tz", vec![n(self.scale * 100.0)]));
        v.push(Op::new("TL", vec![n(self.leading)]));
        v.push(Op::new("Ts", vec![n(self.rise)]));
        v.extend(self.fill.iter().cloned());
        v
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Origin {
    tm: [f64; 6],
    /// The text line matrix there (where the next line's Td/T* starts from).
    tlm: [f64; 6],
    /// Text-space units → user space (the scale of the text rendering matrix's y axis).
    k: f64,
    /// The CTM there (current space → page space), for moving the paragraph in page space.
    ctm: [f64; 6],
    baseline: f64,
    x: f64,
    state: TextState,
    bt_op: usize,
    bt_state: TextState,
    bt_scope: Option<usize>,
}

/// A paragraph: consecutive lines in one stream with the same font and size, aligned on the left
/// and regularly spaced.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// The lines' text joined with spaces (lines that ended in a hyphen are joined directly).
    pub text: String,
    pub rect: [f64; 4],
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The text fill colour as RGB, used by the visual editor and preserved when no colour
    /// override is requested.
    pub color: [f64; 3],
    /// Indexes into [`text_lines`].
    pub lines: Vec<usize>,
}

impl TextLine {
    /// The baseline's height in user space.
    pub fn origin_baseline(&self) -> f64 {
        self.origin.baseline
    }
}

/// What replacing a line did.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    /// `Some(font)` when the line's font couldn't show the new text and Helvetica was used.
    pub substituted: Option<String>,
}

#[derive(Clone)]
struct Ts {
    ctm: Matrix,
    /// The operators that set the current fill colour (`g`, `rg`, `k`, or `cs` + `sc`/`scn`).
    fill: Vec<Op>,
    font: Option<(Vec<u8>, Rc<Metrics>)>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
}

fn page_dict(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn fill_color(fill: &[Op]) -> [f64; 3] {
    let Some(op) = fill.iter().rev().find(|op| matches!(op.op.as_slice(), b"g" | b"rg" | b"k")) else {
        return [0.0, 0.0, 0.0];
    };
    match op.op.as_slice() {
        b"g" => {
            let v = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
            [v, v, v]
        }
        b"rg" => [
            op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
        ],
        b"k" => {
            let c = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0);
            let m = op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0);
            let y = op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0);
            let k = op.operands.get(3).and_then(Object::as_f64).unwrap_or(0.0);
            [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
        }
        _ => [0.0, 0.0, 0.0],
    }
}

/// The text-showing operators of one stream with their text, font, box and baseline.
struct Shown {
    op: usize,
    /// The text matrix where it starts (text space), and the state there and at its `BT`.
    tm: Matrix,
    tlm: Matrix,
    state: Ts,
    bt_op: usize,
    bt_state: Ts,
    scope: Option<usize>,
    bt_scope: Option<usize>,
    text: String,
    rect: [f64; 4],
    baseline: f64,
    start_x: f64,
    end_x: f64,
    bt: usize,
    font: Vec<u8>,
    base_font: String,
    size: f64,
    bold: bool,
    italic: bool,
    decodable: bool,
}

/// The graphics state carried from one of a page's content streams to the next: the streams
/// are one stream in pieces (§7.8.2), so a `cm` (AutoCAD scales the whole page in the first
/// stream), an unbalanced `q`, the font and the colour still apply in the streams after it.
struct Carry {
    ts: Ts,
    stack: Vec<Ts>,
}

impl Carry {
    fn new() -> Self {
        let ts = Ts {
            ctm: Matrix::IDENTITY,
            fill: Vec::new(),
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        };
        Carry { ts, stack: Vec::new() }
    }
}

fn interpret(
    doc: &Document,
    ops: &[Op],
    fonts_res: &Dict,
    cache: &mut HashMap<Vec<u8>, Rc<Metrics>>,
    carry: &mut Carry,
) -> Result<Vec<Shown>, EditError> {
    let mut out = Vec::new();
    let mut ts = carry.ts.clone();
    let mut at_bt = (0usize, ts.clone(), None);
    let mut scopes = Vec::new();
    let mut stack: Vec<Ts> = std::mem::take(&mut carry.stack);
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let mut bt = 0usize;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BMC" | b"BDC" => {
                if scopes.len() >= 256 {
                    return Err(EditError::Invalid("marked-content nesting exceeds the editing limit of 256".into()));
                }
                scopes.push(i);
            }
            b"EMC" => {
                scopes.pop();
            }
            b"q" => stack.push(ts.clone()),
            b"Q" => {
                if let Some(s) = stack.pop() {
                    ts = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ts.ctm = Matrix(m).then(&ts.ctm);
                }
            }
            b"BT" => {
                tm = Matrix::IDENTITY;
                tlm = Matrix::IDENTITY;
                bt += 1;
                at_bt = (i, ts.clone(), scopes.last().copied());
            }
            b"g" | b"rg" | b"k" | b"cs" => ts.fill = vec![op.clone()],
            b"sc" | b"scn" => {
                ts.fill.retain(|o| o.is("cs"));
                ts.fill.push(op.clone());
            }
            b"Tf" => {
                ts.size = op.num(1).unwrap_or(ts.size);
                if let Some(name) = op.name(0) {
                    let m = cache.entry(name.to_vec()).or_insert_with(|| {
                        Rc::new(
                            fonts_res
                                .get(name)
                                .and_then(|f| doc.resolve(f).as_dict().cloned())
                                .map(|d| Metrics::from_dict(doc, &d))
                                .unwrap_or_else(Metrics::fallback),
                        )
                    });
                    ts.font = Some((name.to_vec(), m.clone()));
                }
            }
            b"Tc" => ts.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => ts.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => ts.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => ts.leading = op.num(0).unwrap_or(0.0),
            b"Ts" => ts.rise = op.num(0).unwrap_or(0.0),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.is("TD") {
                        ts.leading = -y;
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, x, y]).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    tlm = Matrix(m);
                    tm = tlm;
                }
            }
            b"T*" => {
                tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    if op.is("\"") {
                        ts.word_spacing = op.num(0).unwrap_or(ts.word_spacing);
                        ts.char_spacing = op.num(1).unwrap_or(ts.char_spacing);
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                    tm = tlm;
                }
                let Some((name, m)) = ts.font.clone() else { continue };
                // Pieces: strings, and TJ number adjustments.
                let pieces: Vec<Object> = match op.op.as_slice() {
                    b"TJ" => op.operands.first().and_then(Object::as_array).cloned().unwrap_or_default(),
                    _ => op.operands.last().cloned().into_iter().collect(),
                };
                let trm0 = tm.then(&ts.ctm);
                let start = trm0.apply(0.0, ts.rise);
                let mut text = String::new();
                let mut decodable = true;
                let mut x_text = 0.0;
                for p in &pieces {
                    match p {
                        Object::String(s) => {
                            for (code, len) in m.codes(&s.bytes) {
                                match m.text_of(code) {
                                    Some(t) => text.push_str(t),
                                    None => decodable = false,
                                }
                                let w = m.width(code) * ts.size + ts.char_spacing + if m.is_space(code, len) { ts.word_spacing } else { 0.0 };
                                x_text += w * ts.scale;
                            }
                        }
                        other => {
                            if let Some(n) = other.as_f64() {
                                let dx = -n / 1000.0 * ts.size * ts.scale;
                                // A large gap inside TJ reads as a space.
                                if dx > ts.size * 0.2 && !text.ends_with(' ') {
                                    text.push(' ');
                                }
                                x_text += dx;
                            }
                        }
                    }
                }
                tm = Matrix([1.0, 0.0, 0.0, 1.0, x_text, 0.0]).then(&tm);
                let end = tm.then(&ts.ctm).apply(0.0, ts.rise);
                // The box: from descent to ascent along the run.
                let corners = [
                    trm0.apply(0.0, ts.rise + m.descent * ts.size),
                    trm0.apply(x_text, ts.rise + m.descent * ts.size),
                    trm0.apply(0.0, ts.rise + m.ascent * ts.size),
                    trm0.apply(x_text, ts.rise + m.ascent * ts.size),
                ];
                let rect = corners
                    .iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
                let size_user = (trm0.0[2].powi(2) + trm0.0[3].powi(2)).sqrt() * ts.size;
                out.push(Shown {
                    op: i,
                    tm: Matrix([tm.0[0], tm.0[1], tm.0[2], tm.0[3], tm.0[4] - x_text * tm.0[0], tm.0[5] - x_text * tm.0[1]]),
                    state: ts.clone(),
                    tlm,
                    bt_op: at_bt.0,
                    bt_state: at_bt.1.clone(),
                    scope: scopes.last().copied(),
                    bt_scope: at_bt.2,
                    text,
                    rect,
                    baseline: start.1,
                    start_x: start.0,
                    end_x: end.0,
                    bt,
                    font: name,
                    base_font: m.base_font.clone(),
                    size: if size_user > 0.0 { size_user } else { ts.size },
                    bold: m.bold,
                    italic: m.italic,
                    decodable,
                });
            }
            _ => {}
        }
    }
    carry.ts = ts;
    carry.stack = stack;
    Ok(out)
}

/// The lines of text on a page (0-based), in content order.
pub fn text_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let mut cache = HashMap::new();
    let mut lines: Vec<TextLine> = Vec::new();
    let mut carry = Carry::new();
    let content = Content::read(doc, &p.dict)?;
    {
        let shown = interpret(doc, &content.ops, &fonts_res, &mut cache, &mut carry)?;
        let mut last: Option<(usize, f64, f64, f64)> = None; // (bt, baseline, end_x, size)
        for s in shown {
            let si = content.piece_of(s.op);
            let joins = last.is_some_and(|(bt, base, end, size)| {
                bt == s.bt
                    && lines.last().is_some_and(|l| l.scope == s.scope)
                    && lines.last().is_some_and(|l| l.font_name == s.font)
                    && lines.last().is_some_and(|l| {
                        (s.size - l.size).abs() < 0.01 && s.bold == l.bold && s.italic == l.italic && fill_color(&s.state.fill) == l.color
                    })
                    && (s.baseline - base).abs() < size * 0.3
                    && s.start_x > end - size
                    && s.start_x - end < size * 3.0
            });
            if joins && let Some(l) = lines.last_mut() {
                let gap = s.start_x - last.map_or(s.start_x, |x| x.2);
                if gap > l.size * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
                    l.text.push(' ');
                }
                l.text.push_str(&s.text);
                l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
                l.ops.push(s.op);
                l.decodable &= s.decodable;
                l.color = fill_color(&s.state.fill);
            } else {
                lines.push(TextLine {
                    text: s.text.clone(),
                    rect: s.rect,
                    font: String::from_utf8_lossy(&s.font).into_owned(),
                    font_name: s.font.clone(),
                    base_font: s.base_font.clone(),
                    size: s.size,
                    bold: s.bold,
                    italic: s.italic,
                    color: fill_color(&s.state.fill),
                    decodable: s.decodable,
                    stream: si,
                    scope: s.scope,
                    ops: vec![s.op],
                    origin: Origin {
                        tm: s.tm.0,
                        tlm: s.tlm.0,
                        k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                        ctm: s.state.ctm.0,
                        baseline: s.baseline,
                        x: s.start_x,
                        state: TextState::of(&s.state),
                        bt_op: s.bt_op,
                        bt_state: TextState::of(&s.bt_state),
                        bt_scope: s.bt_scope,
                    },
                });
            }
            last = Some((s.bt, s.baseline, s.end_x, s.size.max(1.0)));
        }
    }
    // Lines of only spaces aren't editable text.
    lines.retain(|l| !l.text.trim_matches(|c: char| c.is_ascii_whitespace()).is_empty());
    Ok(lines)
}

/// Text → the bytes that show it in the chosen font.
type Encoder = Box<dyn Fn(&str) -> Option<Vec<u8>>>;

fn is_win_ansi_char(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' | '€' | '‚' | '„' | '…' | '‘' | '’' | '“' | '”' | '•' | '–' | '—' | '™' | '\t')
}

fn needs_type3(text: &str) -> bool {
    text.chars().any(|c| !is_win_ansi_char(c))
}

fn source_family(base_font: &str) -> crate::added::Family {
    let name = base_font.to_ascii_lowercase();
    if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        crate::added::Family::Courier
    } else if !name.contains("sans") && ["times", "serif", "roman", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s)) {
        crate::added::Family::Times
    } else {
        crate::added::Family::Helvetica
    }
}

const SUBSTITUTE_NAME: &str = "PCEdHelv";
fn type3_font(doc: &mut Document, fonts_res: &mut Dict, text: &str) -> Result<Type3Fallback, EditError> {
    pdfcraft_fonts::type3_font(doc, fonts_res, text).map_err(EditError::Invalid)
}

/// Quote-show operators also change text state and move the line matrix. Removing
/// their painted text must leave those effects for later, untouched operators.
fn quote_side_effects(op: &Op) -> Vec<Op> {
    match op.op.as_slice() {
        b"'" => vec![Op::new("T*", vec![])],
        b"\"" => vec![
            Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Object::Int(0))]),
            Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Object::Int(0))]),
            Op::new("T*", vec![]),
        ],
        _ => Vec::new(),
    }
}

/// Replace the text of line `line` (an index into [`text_lines`]) on `page` with `text`.
pub fn replace_line(doc: &mut Document, page: usize, line: usize, text: &str) -> Result<LineEdit, EditError> {
    let mut staged = doc.clone();
    let result = replace_line_inner(&mut staged, page, line, text)?;
    *doc = staged;
    Ok(result)
}

fn replace_line_inner(doc: &mut Document, page: usize, line: usize, text: &str) -> Result<LineEdit, EditError> {
    let text = text.replace(['\n', '\r'], " ");
    let lines = text_lines(doc, page)?;
    let target = lines.get(line).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no line {}", page + 1, line + 1)))?;
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let content = Content::read(doc, &p.dict)?;
    let ops = &content.ops;
    let first = target.ops.first().copied().ok_or_else(|| EditError::Invalid("the line has no text operators".into()))?;
    let first_op = ops.get(first).ok_or_else(|| EditError::Invalid("the page's content changed".into()))?;
    // The line's own font, when it can show every character.
    let font = fonts_res.get(&target.font_name).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let reused = font.as_ref().and_then(|m| m.encode(&text));
    let mut substituted = None;
    let mut replacement = quote_side_effects(first_op);
    match reused {
        Some(bytes) => replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))])),
        None => {
            if needs_type3(&text) {
                let fallback = type3_font(doc, &mut fonts_res, &text)?;
                let bytes = type3_encode(&fallback, &text)
                    .ok_or_else(|| EditError::Invalid(format!("\"{text}\" can't be shown by the Japanese fallback")))?;
                let size = target.origin.state.font.as_ref().map_or(target.size, |(_, size)| *size);
                replacement.push(Op::new("Tf", vec![Object::name(&fallback.name), pdfcraft_content::num(size)]));
                replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
                replacement.push(Op::new("Tf", vec![Object::Name(target.font_name.clone()), pdfcraft_content::num(size)]));
                substituted = Some(format!("{} Type3", fallback.family));
            } else {
                let win = pdfcraft_fonts::win_ansi(&text);
                // WinAnsi turns what it can't show into '?'; refuse rather than print the wrong thing.
                let back: String = win.iter().map(|b| char::from_u32(u32::from(*b)).unwrap_or('?')).collect();
                if text.chars().zip(back.chars()).any(|(a, b)| b == '?' && a != '?') {
                    return Err(EditError::Invalid(format!("\"{text}\" has characters neither {} nor Helvetica can show", target.base_font)));
                }
                // The size in text space: the current Tf's size.
                let size = target.origin.state.font.as_ref().map_or(target.size, |(_, size)| *size);
                let family = source_family(&target.base_font);
                let base = family.base_font(target.bold, target.italic);
                let font = pdfcraft_fonts::generated::standard_font(base);
                let substitute_name = crate::resource_names::reserve(&fonts_res, &format!("PCEd{}", base.replace('-', "")), &font);
                fonts_res.set(substitute_name.as_bytes().to_vec(), font);
                replacement.push(Op::new("Tf", vec![Object::name(&substitute_name), pdfcraft_content::num(size)]));
                replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(win))]));
                replacement.push(Op::new("Tf", vec![Object::Name(target.font_name.clone()), pdfcraft_content::num(size)]));
                substituted = Some(base.to_string());
            }
        }
    }
    // Rebuild: the line's first operator becomes the replacement, its others go. Copies of the
    // matching text in the same marked-content scope go too, leaving one replacement.
    let mut drops = coincident_ops(&lines, &[&target]);
    drops.extend(target.ops.iter().copied());
    let contents = content.rewrite(doc, |i| {
        if i == first {
            (std::mem::take(&mut replacement), false)
        } else if drops.contains(&i) {
            // Later shows, including coincident duplicates, can still change spacing or
            // the line matrix. Preserve those effects in their original operator order.
            (ops.get(i).map(quote_side_effects).unwrap_or_default(), false)
        } else {
            (Vec::new(), true)
        }
    })?;
    let page_ref = p.obj;
    if substituted.is_some() {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if substituted.is_some() {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted })
}

/// Only matching text in the same marked-content scope can be a duplicate. Geometric
/// overlap alone also describes unrelated captions or alternative language/layer content.
fn coincident_ops(lines: &[TextLine], targets: &[&TextLine]) -> std::collections::HashSet<usize> {
    let mut drop = std::collections::HashSet::new();
    for line in lines {
        let duplicate = targets.iter().any(|target| {
            line.decodable
                && target.decodable
                && line.text == target.text
                && line.scope == target.scope
                && line.font_name == target.font_name
                && line.base_font == target.base_font
                && line.bold == target.bold
                && line.italic == target.italic
                && line.color == target.color
                && (line.size - target.size).abs() < 0.01
                && line.rect.iter().zip(target.rect).all(|(a, b)| (a - b).abs() < target.size.abs() * 0.1)
        });
        if duplicate {
            drop.extend(line.ops.iter().copied());
        }
    }
    drop
}

/// The paragraphs on a page (0-based).
pub fn text_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines))
}

/// Lines that share a left edge, a centre or a right edge (left, centred or right-aligned text).
fn aligned(a: &TextLine, b: &TextLine) -> bool {
    let tol = b.size.max(1.0);
    let centre = |r: [f64; 4]| (r[0] + r[2]) / 2.0;
    (a.origin.x - b.origin.x).abs() < tol || (centre(a.rect) - centre(b.rect)).abs() < tol || (a.rect[2] - b.rect[2]).abs() < tol
}

fn group_blocks(lines: &[TextLine]) -> Vec<TextBlock> {
    let mut blocks: Vec<TextBlock> = Vec::new();
    let mut gap: Option<f64> = None;
    for (i, l) in lines.iter().enumerate() {
        let joins = i > 0
            && blocks.last().is_some_and(|b| {
                let Some(&p) = b.lines.last() else { return false };
                let prev = &lines[p];
                let g = prev.origin.baseline - l.origin.baseline;
                prev.stream == l.stream
                    && prev.scope == l.scope
                    && prev.font_name == l.font_name
                    && prev.base_font == l.base_font
                    && prev.bold == l.bold
                    && prev.italic == l.italic
                    && prev.color == l.color
                    && (prev.size - l.size).abs() < 0.01
                    && aligned(prev, l)
                    && g > l.size * 0.8
                    && g < l.size * 2.5
                    && gap.is_none_or(|first| (g - first).abs() < first * 0.2)
            });
        if joins
            && let Some(b) = blocks.last_mut()
            && let Some(&p) = b.lines.last()
        {
            let prev = &lines[p];
            gap.get_or_insert(prev.origin.baseline - l.origin.baseline);
            if b.text.ends_with('-') {
                b.text.pop();
            } else {
                b.text.push(' ');
            }
            b.text.push_str(l.text.trim_matches(|c: char| c.is_ascii_whitespace()));
            b.rect = [b.rect[0].min(l.rect[0]), b.rect[1].min(l.rect[1]), b.rect[2].max(l.rect[2]), b.rect[3].max(l.rect[3])];
            b.lines.push(i);
        } else {
            gap = None;
            blocks.push(TextBlock {
                text: l.text.trim_matches(|c: char| c.is_ascii_whitespace()).to_string(),
                rect: l.rect,
                base_font: l.base_font.clone(),
                size: l.size,
                bold: l.bold,
                italic: l.italic,
                color: l.color,
                lines: vec![i],
            });
        }
    }
    blocks
}

/// Greedy word wrapping to `width` with `advance` giving a string's width.
fn wrap(text: &str, width: f64, advance: impl Fn(&str) -> f64) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_ascii_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && advance(&candidate) > width {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// Replace paragraph `block` (an index into [`text_blocks`]) with `text`, rewrapped to the
/// paragraph's width with its line spacing. The paragraph keeps its first line's position, font
/// (or Helvetica when the font can't show the text), size and colour.
pub fn replace_block(doc: &mut Document, page: usize, block: usize, text: &str) -> Result<LineEdit, EditError> {
    rewrite_block(doc, page, block, Some(text), &BlockStyle::default())
}

/// Formatting for a rewritten paragraph (`None` keeps the paragraph's own).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStyle {
    /// A standard font family, bold, italic.
    pub family: Option<(crate::added::Family, bool, bool)>,
    /// Font size in points (user space).
    pub size: Option<f64>,
    /// Fill colour (RGB 0–1).
    pub color: Option<[f64; 3]>,
    pub align: Option<crate::added::Align>,
    /// Draw a line under each line of text.
    pub underline: Option<bool>,
    /// Line spacing as a multiple of the font size (1.2 is ordinary).
    pub line_spacing: Option<f64>,
    /// Character spacing (points) and horizontal scaling (percent).
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
    /// Move the paragraph by `[dx, dy]` in user space (dragging its box).
    pub offset: Option<[f64; 2]>,
    /// Rewrap to this width in user space (dragging the box's edge).
    pub width: Option<f64>,
}

/// Rewrite paragraph `block` with new text (or its own) and formatting, rewrapped to its width.
pub fn rewrite_block(doc: &mut Document, page: usize, block: usize, text: Option<&str>, style: &BlockStyle) -> Result<LineEdit, EditError> {
    // Font fallback and stream rewrites allocate objects before every fallible step has
    // finished. Commit the snapshot only after the whole direct API operation succeeds.
    let mut staged = doc.clone();
    let result = rewrite_block_inner(&mut staged, page, block, text, style)?;
    *doc = staged;
    Ok(result)
}

fn rewrite_block_inner(doc: &mut Document, page: usize, block: usize, text: Option<&str>, style: &BlockStyle) -> Result<LineEdit, EditError> {
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let b = blocks.get(block).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no paragraph {}", page + 1, block + 1)))?;
    let members: Vec<&TextLine> = b.lines.iter().map(|i| &lines[*i]).collect();
    let first = members[0];
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let content = Content::read(doc, &p.dict)?;
    let ops = &content.ops;
    let text = text.unwrap_or(&b.text).split_ascii_whitespace().collect::<Vec<_>>().join(" ");
    let o = &first.origin;
    let (font_name, old_size) = o.state.font.clone().ok_or_else(|| EditError::Invalid("the paragraph has no font".into()))?;
    // Paragraph relocation must not move text out of its layer/tag, or move BT/ET through
    // a marked-content sequence that began inside the original text object. Line edits can
    // still replace these runs without relocating their enclosing operators.
    if first.scope != o.bt_scope {
        return Err(EditError::Invalid(
            "paragraph reflow inside marked content opened within BT is unsupported; edit individual lines instead".into(),
        ));
    }
    if !o.k.is_finite() || o.k <= 0.0 || !old_size.is_finite() || old_size <= 0.0 || old_size * o.k > 14_400.0 {
        return Err(EditError::Invalid("the paragraph's source font size must be finite and between 0 and 14400 points".into()));
    }
    if style.size.is_some_and(|pt| !pt.is_finite() || pt <= 0.0 || pt > 14_400.0) {
        return Err(EditError::Invalid("font size must be finite and between 0 and 14400 points".into()));
    }
    let k = o.k.max(1e-6);
    // The size in text space: points ÷ the text-to-user scale.
    let size = style.size.map_or(old_size, |pt| (pt / k).max(0.1));
    if !size.is_finite() || !(size * k).is_finite() || size * k > 14_400.0 {
        return Err(EditError::Invalid("the paragraph's effective font size exceeds 14400 points".into()));
    }
    // Spacing and scaling (text state), with the chosen values.
    let mut ts_state = o.state.clone();
    if let Some(c) = style.char_spacing {
        ts_state.char_spacing = c / k;
    }
    if let Some(sc) = style.scale {
        ts_state.scale = (sc / 100.0).clamp(0.1, 10.0);
    }
    let o_state = ts_state.clone();
    let metrics = fonts_res.get(&font_name).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let reuse = style.family.is_none() && metrics.as_ref().is_some_and(|m| m.encode(&text).is_some());
    let type3 = if !reuse && needs_type3(&text) { Some(type3_font(doc, &mut fonts_res, &text)?) } else { None };
    // The standard font used when the paragraph's own can't be (chosen, or substituted).
    let (family, bold, italic) = style.family.unwrap_or((source_family(&b.base_font), b.bold, b.italic));
    let std_width = move |s: &str, size: f64| -> f64 {
        match family {
            crate::added::Family::Courier => s.chars().count() as f64 * 0.6 * size,
            crate::added::Family::Times => pdfcraft_fonts::helvetica_width(s, size) * 0.92,
            crate::added::Family::Helvetica => pdfcraft_fonts::helvetica_width(s, size) * if bold { 1.05 } else { 1.0 },
        }
    };
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(EditError::Invalid("the paragraph can't be moved that far".into()));
    }
    // A paragraph keeps its width (or takes the one asked for); a single line grows to the right,
    // up to the page's margin.
    let width = match style.width {
        // Capped at the largest page PDF allows (14 400 pt).
        Some(w) if w.is_finite() => w.clamp(size * k, 14_400.0),
        Some(_) => return Err(EditError::Invalid("the paragraph's width must be a number".into())),
        None if members.len() == 1 => (b.rect[2] - b.rect[0]).max(size * k).max(p.crop(doc)[2] - 36.0 - (b.rect[0] + dx)),
        None => (b.rect[2] - b.rect[0]).max(size * k),
    };
    let advance = |s: &str| -> f64 {
        let t = match (&metrics, reuse) {
            (Some(m), true) => m
                .encode(s)
                .map(|bytes| {
                    m.codes(&bytes)
                        .iter()
                        .map(|(c, l)| m.width(*c) * size + o_state.char_spacing + if m.is_space(*c, *l) { o_state.word_spacing } else { 0.0 })
                        .sum::<f64>()
                })
                .unwrap_or(0.0),
            _ if let Some(fallback) = &type3 => {
                fallback.codes.iter().map(|(ch, _, width)| s.chars().filter(|c| c == ch).count() as f64 * width * size).sum::<f64>()
                    + s.chars().count() as f64 * o_state.char_spacing
            }
            _ => std_width(s, size) + s.chars().count() as f64 * o_state.char_spacing,
        };
        t * o_state.scale * k
    };
    let wrapped = wrap(&text, width + 0.5, advance);
    let mut substituted = None;
    let new_font = !reuse;
    let (show_font, encode): (Vec<u8>, Encoder) = if let Some(m) = metrics.clone().filter(|_| reuse) {
        (font_name.clone(), Box::new(move |s: &str| m.encode(s)))
    } else if let Some(fallback) = type3.clone() {
        let name = fallback.name.clone();
        let encoder = fallback.clone();
        substituted = Some(format!("{} Type3", fallback.family));
        (name.into_bytes(), Box::new(move |s: &str| type3_encode(&encoder, s)))
    } else {
        let win = pdfcraft_fonts::win_ansi(&text);
        let back: String = win.iter().map(|c| char::from_u32(u32::from(*c)).unwrap_or('?')).collect();
        let base = family.base_font(bold, italic);
        if text.chars().zip(back.chars()).any(|(a, c)| c == '?' && a != '?') {
            return Err(EditError::Invalid(format!("\"{text}\" has characters neither {} nor {base} can show", b.base_font)));
        }
        let font = pdfcraft_fonts::generated::standard_font(base);
        let preferred = if style.family.is_none() { SUBSTITUTE_NAME.to_owned() } else { format!("PCEd{}", base.replace('-', "")) };
        let name = crate::resource_names::reserve(&fonts_res, &preferred, &font);
        fonts_res.set(name.as_bytes().to_vec(), font);
        if style.family.is_none() {
            substituted = Some(base.to_string());
        }
        (name.into_bytes(), Box::new(|s: &str| Some(pdfcraft_fonts::win_ansi(s))))
    };
    // Line spacing in text space: the paragraph's own (scaled with the size), or 1.2 × the size.
    let lead = match style.line_spacing {
        Some(m) => size * m.clamp(0.5, 5.0),
        None if members.len() > 1 => (members[0].origin.baseline - members[1].origin.baseline) / k * size / old_size,
        None if o_state.leading > 0.0 => o_state.leading,
        None => size * 1.2,
    };
    let n = pdfcraft_content::num;
    // In its own graphics state, so a new colour (or anything else) stops at the paragraph.
    let mut block_ops = vec![Op::new("q", vec![])];
    // Moved: a translation inside that state. The move is in page space, so it is taken back
    // through the CTM's linear part into the space the paragraph is drawn in.
    if dx != 0.0 || dy != 0.0 {
        let c = o.ctm;
        let back = Matrix([c[0], c[1], c[2], c[3], 0.0, 0.0])
            .invert()
            .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it can't be moved in".into()))?;
        let (ax, ay) = back.apply(dx, dy);
        block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(ax), n(ay)]));
    }
    block_ops.push(Op::new("BT", vec![]));
    let mut state = o_state.clone();
    state.word_spacing = 0.0;
    state.font = Some((show_font, size));
    if let Some([r, g, bl]) = style.color {
        state.fill = vec![Op::new("rg", vec![n(r), n(g), n(bl)])];
    }
    block_ops.extend(state.ops());
    block_ops.push(Op::new("Tm", o.tm.iter().map(|v| n(*v)).collect()));
    // Alignment: each line's offset from the left edge, in text space.
    let offset = |line: &str| -> f64 {
        let free = (width - advance(line)).max(0.0) / k;
        match style.align {
            Some(crate::added::Align::Center) => free / 2.0,
            Some(crate::added::Align::Right) => free,
            _ => 0.0,
        }
    };
    // Justify: word spacing (single-byte code 32 only) so each line but the last fills the width.
    let single_byte = !reuse || metrics.as_ref().is_some_and(|m| !m.composite);
    let justify = style.align == Some(crate::added::Align::Justify) && single_byte;
    let mut x = 0.0;
    let mut tw_set = 0.0;
    let mut underlines: Vec<(f64, f64, f64)> = Vec::new(); // (x0, x1, y) in text space
    for (i, line) in wrapped.iter().enumerate() {
        let dx = offset(line);
        if i > 0 || dx != 0.0 {
            block_ops.push(Op::new("Td", vec![n(dx - x), n(if i > 0 { -lead } else { 0.0 })]));
        }
        x = dx;
        let spaces = line.matches(' ').count();
        let tw =
            if justify && i + 1 < wrapped.len() && spaces > 0 { (width - advance(line)).max(0.0) / k / o_state.scale / spaces as f64 } else { 0.0 };
        if tw != tw_set {
            block_ops.push(Op::new("Tw", vec![n(tw)]));
            tw_set = tw;
        }
        let bytes = encode(line).ok_or_else(|| EditError::Invalid(format!("\"{line}\" can't be shown")))?;
        block_ops.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
        let w = (advance(line) + tw * spaces as f64 * o_state.scale * k) / k;
        underlines.push((dx, dx + w, -(i as f64) * lead - size * 0.12));
    }
    block_ops.push(Op::new("ET", vec![]));
    if style.underline == Some(true) {
        // In text space (the paragraph's text matrix), in its fill colour.
        let colour: Vec<Op> = state
            .fill
            .iter()
            .map(|f| match f.op.as_slice() {
                b"g" => Op::new("G", f.operands.clone()),
                b"rg" => Op::new("RG", f.operands.clone()),
                b"k" => Op::new("K", f.operands.clone()),
                b"cs" => Op::new("CS", f.operands.clone()),
                b"sc" => Op::new("SC", f.operands.clone()),
                _ => Op::new("SCN", f.operands.clone()),
            })
            .collect();
        block_ops.push(Op::new("q", vec![]));
        block_ops.push(Op::new("cm", o.tm.iter().map(|v| n(*v)).collect()));
        block_ops.extend(colour);
        block_ops.push(Op::new("w", vec![n(size * 0.06)]));
        for (x0, x1, y) in underlines {
            block_ops.push(Op::new("m", vec![n(x0), n(y)]));
            block_ops.push(Op::new("l", vec![n(x1), n(y)]));
        }
        block_ops.push(Op::new("S", vec![]));
        block_ops.push(Op::new("Q", vec![]));
    }
    block_ops.push(Op::new("Q", vec![]));
    // Remove the paragraph and matching duplicates in the same marked-content scope
    // (for example fake bold); unrelated overlapping text and other scopes remain.
    let mut drop: std::collections::HashSet<usize> = members.iter().flat_map(|l| l.ops.iter().copied()).collect();
    drop.extend(coincident_ops(&lines, &members));
    // Text shown earlier in the same text object stays first: the paragraph then goes where it
    // was, between two halves of the text object, so it keeps its place in reading order.
    let start = first.ops.first().copied().unwrap_or(o.bt_op);
    let split = ops
        .get(o.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(o.bt_op + i)));
    // What the text after the paragraph expects: the state at its BT (or where it was split).
    let after = if split { &o.state } else { &o.bt_state };
    block_ops.extend(after.ops().into_iter().filter(|op| !op.is("Tf") || after.font.is_some()));
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", o.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { o.bt_op };
    let contents = content.rewrite(doc, |i| {
        let mut insert = if i == at { std::mem::take(&mut block_ops) } else { Vec::new() };
        let removed = drop.contains(&i);
        // A split restores the first show's post-quote state and line matrix above.
        // Every other removed quote must still move the original line matrix and
        // update spacing, or an untouched later run inherits a different state.
        if removed
            && !(split && i == start)
            && let Some(op) = ops.get(i)
        {
            insert.extend(quote_side_effects(op));
        }
        (insert, !removed)
    })?;
    if new_font {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    let page_ref = p.obj;
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if new_font {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted })
}
