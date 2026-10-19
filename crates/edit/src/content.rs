//! Parse a page as one content stream, but patch only the original pieces that change.

use std::collections::HashMap;
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
                Object::Stream(s) => {
                    if s.filters().is_empty() && s.raw.len() > remaining {
                        return Err(EditError::Invalid("page content exceeds the 256 MiB editing limit".into()));
                    }
                    s.decoded_within(remaining)?
                }
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
        let ops = parse(&data).ops;
        Ok(Self { data, pieces, ops })
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
