//! Parse a page as one content stream, but patch only the original pieces that change.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object, Stream};

use crate::EditError;

struct Piece {
    object: Object,
    span: Range<usize>,
}

pub(crate) struct Content {
    data: Vec<u8>,
    pieces: Vec<Piece>,
    pub(crate) ops: Vec<Op>,
}

struct Patch {
    span: Range<usize>,
    insert: Vec<u8>,
}

impl Content {
    pub(crate) fn read(doc: &Document, page: &Dict) -> Result<Self, EditError> {
        let objects = match page.get(b"Contents") {
            None => Vec::new(),
            Some(c) => match &*doc.resolve(c) {
                Object::Array(a) => a.clone(),
                _ => vec![c.clone()],
            },
        };
        let mut data = Vec::new();
        let mut pieces = Vec::new();
        for object in objects {
            let remaining = (256usize * 1024 * 1024).saturating_sub(data.len()).saturating_sub(1);
            let decoded = match &*doc.resolve(&object) {
                Object::Stream(s) => normalized_stream(doc, s)?.decoded_strict_within(remaining)?,
                Object::Null => Vec::new(),
                _ => return Err(EditError::Invalid("the page's Contents contains a non-stream object".into())),
            };
            // Bound the extra joined buffer independently of the number of source streams.
            let extra = decoded.len().checked_add(1).ok_or_else(|| EditError::Invalid("page content is too large to edit".into()))?;
            if data.len().checked_add(extra).is_none_or(|n| n > 256 * 1024 * 1024) {
                return Err(EditError::Invalid("page content exceeds the 256 MiB editing limit".into()));
            }
            data.try_reserve(extra).map_err(|_| EditError::Invalid("not enough memory to edit page content".into()))?;
            let start = data.len();
            data.extend_from_slice(&decoded);
            pieces.push(Piece { object, span: start..data.len() });
            // Stream boundaries separate tokens, including an unterminated trailing comment.
            data.push(b'\n');
        }
        let parsed = parse(&data);
        // Recovery can detach operands from their original spans. Such spans are safe for
        // tolerant viewing, but cannot identify all bytes a destructive edit must replace.
        if parsed.skipped != 0 {
            return Err(EditError::Invalid("the page contains damaged content syntax and cannot be edited safely".into()));
        }
        Ok(Self { data, pieces, ops: parsed.ops })
    }

    /// The piece containing the operator keyword; retained for paragraph grouping.
    pub(crate) fn piece_of(&self, op: usize) -> usize {
        let at = self.ops.get(op).map_or(0, |o| o.span.end.saturating_sub(1));
        self.pieces.partition_point(|p| p.span.end <= at)
    }

    /// Insert before selected operators and optionally remove their complete source spans.
    /// Untouched bytes (including unknown syntax) and untouched stream references are retained.
    /// Changed streams are copied, because another page may share their original objects.
    pub(crate) fn rewrite(&self, doc: &mut Document, mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Result<Object, EditError> {
        let mut patches: HashMap<usize, Vec<Patch>> = HashMap::new();
        for (i, op) in self.ops.iter().enumerate() {
            let (insert, keep) = edit(i);
            if insert.is_empty() && keep {
                continue;
            }
            let first = self.pieces.partition_point(|p| p.span.end <= op.span.start);
            let end = if keep { op.span.start } else { op.span.end };
            // A replacement stays in the keyword's piece, preserving paragraph provenance.
            // Its operands are removed from all preceding pieces. An insertion before an
            // unchanged operator must precede its operands, even when those are split.
            let insert_piece = if keep { first } else { self.piece_of(i) };
            let mut bytes = Vec::new();
            if !insert.is_empty() {
                bytes.push(b'\n');
                bytes.extend_from_slice(&serialize_ops(&insert));
            }
            for (index, piece) in self.pieces.iter().enumerate().skip(first) {
                if index != first && piece.span.start >= end {
                    break;
                }
                let start = op.span.start.max(piece.span.start);
                let stop = end.min(piece.span.end).max(start);
                patches
                    .entry(index)
                    .or_default()
                    .push(Patch { span: start..stop, insert: if index == insert_piece { std::mem::take(&mut bytes) } else { Vec::new() } });
            }
        }
        let mut contents = Vec::with_capacity(self.pieces.len());
        for (i, piece) in self.pieces.iter().enumerate() {
            let Some(edits) = patches.remove(&i) else {
                contents.push(piece.object.clone());
                continue;
            };
            let mut out = Vec::new();
            let mut at = piece.span.start;
            for patch in edits {
                let unchanged = self.data.get(at..patch.span.start).ok_or_else(|| EditError::Invalid("overlapping content edits".into()))?;
                out.extend_from_slice(unchanged);
                out.extend_from_slice(&patch.insert);
                at = patch.span.end;
            }
            out.extend_from_slice(self.data.get(at..piece.span.end).ok_or_else(|| EditError::Invalid("invalid content edit span".into()))?);
            if self.data.get(piece.span.clone()) == Some(out.as_slice()) {
                contents.push(piece.object.clone());
                continue;
            }
            let mut dict = match &*doc.resolve(&piece.object) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            contents.push(Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &out)))));
        }
        Ok(if contents.len() == 1 { contents.pop().unwrap_or(Object::Null) } else { Object::Array(contents) })
    }
}

/// Resolve metadata with bounded reference following. Unlike `Document::get`, this path
/// does not turn a load error or a dangling reference into an absent decode parameter.
fn metadata(doc: &Document, value: &Object) -> Result<Object, EditError> {
    let mut value = value.clone();
    let mut seen = HashSet::new();
    while let Object::Ref(reference) = value {
        if seen.len() >= 32 || !seen.insert(reference) {
            return Err(EditError::Invalid("cyclic or excessively nested stream decode metadata".into()));
        }
        if doc.generation(reference.num) != reference.generation {
            return Err(EditError::Invalid("stream decode metadata references an unavailable object generation".into()));
        }
        value = doc.try_get(reference.num)?.as_ref().clone();
        if matches!(value, Object::Null) && !doc.object_numbers().contains(&reference.num) {
            return Err(EditError::Invalid("stream decode metadata contains an unresolved reference".into()));
        }
    }
    Ok(value)
}

/// Normalize a temporary stream, never the document's original dictionary. The COS codec
/// deliberately accepts malformed declarations for viewing; destructive editing needs typed,
/// resolved declarations so it cannot silently skip a filter or default an invalid predictor.
fn normalized_stream(doc: &Document, source: &Stream) -> Result<Stream, EditError> {
    for key in [b"F".as_slice(), b"FFilter", b"FDecodeParms"] {
        if let Some(value) = source.dict.get(key)
            && !matches!(metadata(doc, value)?, Object::Null)
        {
            return Err(EditError::Invalid("editing externally stored page content is unsupported".into()));
        }
    }
    let filter = source.dict.get(b"Filter").map(|o| metadata(doc, o)).transpose()?.unwrap_or(Object::Null);
    let filters = match filter {
        Object::Null => Vec::new(),
        Object::Name(name) => vec![Object::Name(name)],
        Object::Array(values) if values.len() <= 32 => values.iter().map(|o| metadata(doc, o)).collect::<Result<Vec<_>, _>>()?,
        _ => return Err(EditError::Invalid("stream Filter must be a name or an array of at most 32 names".into())),
    };
    if filters.iter().any(|o| !matches!(o, Object::Name(_))) {
        return Err(EditError::Invalid("stream Filter array contains a non-name value".into()));
    }
    let params = source.dict.get(b"DecodeParms").map(|o| metadata(doc, o)).transpose()?.unwrap_or(Object::Null);
    let params = match params {
        Object::Null => vec![Object::Null; filters.len()],
        Object::Dict(d) if filters.len() == 1 => vec![Object::Dict(d)],
        Object::Array(values) if values.len() == filters.len() => values,
        _ => return Err(EditError::Invalid("stream DecodeParms must match its filter count".into())),
    };
    let mut normalized = Vec::with_capacity(params.len());
    for (filter, params) in filters.iter().zip(params) {
        let params = metadata(doc, &params)?;
        let Object::Dict(params) = params else {
            if matches!(params, Object::Null) {
                normalized.push(Object::Null);
                continue;
            }
            return Err(EditError::Invalid("stream DecodeParms entries must be dictionaries or null".into()));
        };
        let filter = filter.as_name().unwrap_or_default();
        let predictor_filter = matches!(filter, b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW");
        let mut out = Dict::new();
        for (key, value) in params.iter() {
            let value = metadata(doc, value)?;
            // A dictionary entry whose value is null is equivalent to an absent entry.
            if matches!(value, Object::Null) {
                continue;
            }
            let valid = match key.as_slice() {
                b"Predictor" if predictor_filter => value.as_int().is_some_and(|n| matches!(n, 1 | 2 | 10..=15)),
                b"Colors" if predictor_filter => value.as_int().is_some_and(|n| (1..=255).contains(&n)),
                b"BitsPerComponent" if predictor_filter => value.as_int().is_some_and(|n| matches!(n, 1 | 2 | 4 | 8 | 16)),
                b"Columns" if predictor_filter => value.as_int().is_some_and(|n| n > 0),
                b"EarlyChange" if matches!(filter, b"LZWDecode" | b"LZW") => value.as_int().is_some_and(|n| matches!(n, 0 | 1)),
                b"Name" if filter == b"Crypt" => matches!(value, Object::Name(_)),
                b"Type" if filter == b"Crypt" => value.as_name() == Some(b"CryptFilterDecodeParms"),
                _ => return Err(EditError::Invalid(format!("unsupported stream decode parameter {}", String::from_utf8_lossy(key)))),
            };
            if !valid {
                return Err(EditError::Invalid(format!("invalid stream decode parameter {}", String::from_utf8_lossy(key))));
            }
            out.set(key.clone(), value);
        }
        normalized.push(Object::Dict(out));
    }
    let mut stream = source.clone();
    stream.dict.set(b"Filter".to_vec(), Object::Array(filters));
    stream.dict.set(b"DecodeParms".to_vec(), Object::Array(normalized));
    Ok(stream)
}
