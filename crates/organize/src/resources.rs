//! Page-local image pruning before importing a shared resource dictionary.
//!
//! Split / Extract (#204) copies only images an understood page draws, while giving each
//! page its own resource dictionary before import. Only completely understood content is
//! eligible. Forms, Type3 fonts, patterns, graphics states, inline images and annotation
//! appearances can consume resources in additional scopes; leave the original dictionary
//! intact for those pages instead of guessing at their transitive dependencies.

use std::collections::HashSet;

use pdfcraft_content::Op;
use pdfcraft_cos::{Dict, Document, Object};

const MAX_CONTENT: usize = 4 * 1024 * 1024;

pub(crate) fn prune_images(doc: &Document, page: &Dict, inherited: &Dict) -> Option<Object> {
    if page.contains(b"Annots") || !doc.repair_log().is_empty() {
        return None;
    }
    let resources = doc.resolve(page.get(b"Resources").or_else(|| inherited.get(b"Resources"))?);
    let resources = resources.as_dict()?;
    // Extension resource categories may have dependencies we cannot interpret.
    if resources.iter().any(|(k, _)| {
        !matches!(k.as_slice(), b"XObject" | b"Font" | b"ColorSpace" | b"ExtGState" | b"Pattern" | b"Shading" | b"Properties" | b"ProcSet")
    }) {
        return None;
    }
    // A Type3 font without its own resources can draw through the page dictionary. Keep
    // upstream's conservative contract even if no Tf explicitly selects that font here.
    if let Some(fonts) = resources.get(b"Font") {
        let fonts = doc.resolve(fonts);
        if fonts
            .as_dict()?
            .iter()
            .any(|(_, font)| doc.resolve(font).as_dict().is_some_and(|font| font.name(b"Subtype") == Some(b"Type3") && !font.contains(b"Resources")))
        {
            return None;
        }
    }
    let xobjects = doc.resolve(resources.get(b"XObject")?);
    let xobjects = xobjects.as_dict()?;
    let contents = doc.resolve(page.get(b"Contents")?);
    let streams = match contents.as_ref() {
        Object::Stream(_) => vec![contents.as_ref().clone()],
        Object::Array(a) if a.len() <= 4096 => a.clone(),
        _ => return None,
    };
    let mut data = Vec::new();
    let mut boundaries = Vec::new();
    for stream in streams {
        let stream = doc.resolve(&stream);
        let Object::Stream(stream) = stream.as_ref() else { return None };
        // The ordinary stream decoder deliberately tolerates malformed filter
        // declarations and parameter values. Resource removal must not rely on
        // that repair; nonempty DecodeParms need a stricter parameter reader.
        if stream.dict.contains(b"F")
            || stream.dict.contains(b"FFilter")
            || stream.dict.contains(b"FDecodeParms")
            || !match stream.dict.get(b"Filter") {
                None | Some(Object::Null | Object::Name(_)) => true,
                Some(Object::Array(a)) => a.iter().all(|o| matches!(o, Object::Name(_))),
                _ => false,
            }
            || !match stream.dict.get(b"DecodeParms") {
                None | Some(Object::Null) => true,
                Some(Object::Dict(d)) => d.is_empty(),
                Some(Object::Array(a)) => a.iter().all(|o| matches!(o, Object::Null) || o.as_dict().is_some_and(Dict::is_empty)),
                _ => false,
            }
        {
            return None;
        }
        let remaining = MAX_CONTENT.checked_sub(data.len())?;
        if stream.raw.len() > MAX_CONTENT {
            return None;
        }
        let decoded = stream.decoded_strict_within(remaining).ok()?;
        if decoded.len() > remaining {
            return None;
        }
        data.extend_from_slice(&decoded);
        boundaries.push(data.len());
    }
    let parsed = pdfcraft_content::parse(&data);
    if parsed.skipped != 0 {
        return None;
    }
    // Conforming stream boundaries lie between lexical tokens. Tolerant consumers also
    // encounter splits within tokens: hayro inserts spaces, while editing inserts newlines.
    // Pruning must not depend on an ambiguous boundary interpretation. Keep all resources
    // if any interpretation changes an operand/operator or encounters unreadable bytes.
    for separator in *b" \n" {
        let mut separated = Vec::with_capacity(data.len().saturating_add(boundaries.len()));
        let mut at = 0;
        for &end in &boundaries {
            separated.extend_from_slice(data.get(at..end)?);
            separated.push(separator);
            at = end;
        }
        let other = pdfcraft_content::parse(&separated);
        if other.skipped != 0
            || other.ops.len() != parsed.ops.len()
            || !parsed.ops.iter().zip(&other.ops).all(|(a, b)| a.op == b.op && a.operands == b.operands && a.inline == b.inline)
        {
            return None;
        }
    }
    let mut used = HashSet::new();
    let mut depth = 0usize;
    let mut in_text = false;
    for op in &parsed.ops {
        match op.op.as_slice() {
            b"q" if op.operands.is_empty() && !in_text && depth < 256 => depth += 1,
            b"Q" if op.operands.is_empty() && !in_text && depth > 0 => depth -= 1,
            b"BT" if op.operands.is_empty() && !in_text => in_text = true,
            b"ET" if op.operands.is_empty() && in_text => in_text = false,
            b"Do" if op.operands.len() == 1 && !in_text => {
                let name = op.name(0)?;
                let object = doc.resolve(xobjects.get(name)?);
                let Object::Stream(image) = object.as_ref() else { return None };
                if image.dict.name(b"Subtype") != Some(b"Image") {
                    return None;
                }
                used.insert(name.to_vec());
            }
            b"Tf" if in_text && op.operands.len() == 2 && op.num(1).is_some_and(f64::is_finite) => {
                let fonts = doc.resolve(resources.get(b"Font")?);
                let font = doc.resolve(fonts.as_dict()?.get(op.name(0)?)?);
                if !matches!(font.as_dict()?.name(b"Subtype"), Some(b"Type1" | b"MMType1" | b"TrueType" | b"Type0")) {
                    return None;
                }
            }
            _ if ordinary_operator(op, in_text) => {}
            _ => return None,
        }
    }
    if depth != 0 || in_text {
        return None;
    }
    let mut kept = xobjects.clone();
    for (name, value) in xobjects.iter() {
        if !used.contains(name) {
            let value = doc.resolve(value);
            if matches!(value.as_ref(), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")) {
                kept.remove(name);
            }
        }
    }
    if kept.len() == xobjects.len() {
        return None;
    }
    let mut resources = resources.clone();
    resources.set(b"XObject".to_vec(), Object::Dict(kept));
    Some(Object::Dict(resources))
}

fn number(o: &Object) -> bool {
    o.as_f64().is_some_and(f64::is_finite)
}

fn ordinary_operator(op: &Op, in_text: bool) -> bool {
    let args = &op.operands;
    let numeric = |n| args.len() == n && args.iter().all(number);
    match op.op.as_slice() {
        b"cm" if !in_text => numeric(6),
        b"w" | b"J" | b"j" | b"M" | b"i" | b"G" | b"g" => numeric(1),
        b"RG" | b"rg" => numeric(3),
        b"K" | b"k" => numeric(4),
        b"m" | b"l" if !in_text => numeric(2),
        b"c" if !in_text => numeric(6),
        b"v" | b"y" | b"re" if !in_text => numeric(4),
        b"h" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" | b"W" | b"W*" if !in_text => args.is_empty(),
        b"d" => matches!(args.as_slice(), [Object::Array(a), phase] if a.iter().all(number) && number(phase)),
        b"ri" => matches!(args.as_slice(), [Object::Name(_)]),
        b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts" if in_text => numeric(1),
        b"Td" | b"TD" if in_text => numeric(2),
        b"Tm" if in_text => numeric(6),
        b"T*" if in_text => args.is_empty(),
        b"Tj" | b"'" if in_text => matches!(args.as_slice(), [Object::String(_)]),
        b"\"" if in_text => matches!(args.as_slice(), [a, b, Object::String(_)] if number(a) && number(b)),
        b"TJ" if in_text => matches!(args.as_slice(), [Object::Array(a)] if a.iter().all(|o| matches!(o, Object::String(_)) || number(o))),
        // No CS/cs, SCN/scn, gs, sh, marked content, or extension operators:
        // those need a wider resource dependency analysis before pruning.
        _ => false,
    }
}

#[cfg(test)]
mod tests;
