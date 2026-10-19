//! Flatten (Acrobat: Fill & Sign ▸ flatten, Print production ▸ Flattener, Prepare form ▸
//! Flatten): annotation appearances become ordinary page content and the annotations go away.
//!
//! Each visible annotation's normal appearance (for check boxes and radio buttons, the one its
//! `/AS` selects) is drawn into the page with the matrix of ISO 32000-2 §12.5.5 (Algorithm 8.1:
//! the appearance's bounding box, transformed by its `/Matrix`, mapped onto `/Rect`). Hidden
//! annotations are left alone; links are kept (they have no ink and stay clickable). Flattened
//! comments take their pop-ups and replies with them; flattened widgets are removed from the
//! form's field tree.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{EditError, check, page_list, place_tagged};

const HIDDEN: i64 = 2;
const NO_VIEW: i64 = 32;

fn nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|x| doc.resolve(x).as_f64().filter(|v| v.is_finite())).collect()
}

/// The appearance stream to draw for an annotation, if it has one.
fn appearance(doc: &Document, d: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(d.get(b"AP")?);
    let n = ap.as_dict()?.get(b"N")?.clone();
    match &n {
        Object::Ref(r) if matches!(&*doc.get(*r), Object::Stream(_)) => Some(*r),
        _ => {
            let states = doc.resolve(&n);
            let selected = doc.resolve(d.get(b"AS")?);
            let state = selected.as_name()?;
            states.as_dict()?.get(state)?.as_ref().filter(|r| matches!(&*doc.get(*r), Object::Stream(_)))
        }
    }
}

/// Algorithm 8.1: the matrix that maps the form's bounding box onto the annotation rectangle.
fn placement(doc: &Document, form: &Dict, rect: [f64; 4]) -> Option<[f64; 6]> {
    let bbox = nums(doc, form.get(b"BBox")).filter(|b| b.len() == 4)?;
    let m = match form.get(b"Matrix") {
        Some(o) if !matches!(&*doc.resolve(o), Object::Null) => nums(doc, Some(o)).filter(|m| m.len() == 6)?,
        _ => vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };
    if !bbox.iter().chain(&m).chain(&rect).all(|v| v.is_finite()) || rect[2] <= rect[0] || rect[3] <= rect[1] {
        return None;
    }
    let determinant = m[0] * m[3] - m[1] * m[2];
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let pts = [(bbox[0], bbox[1]), (bbox[2], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[3])]
        .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
    let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    if !pts.iter().all(|p| p.0.is_finite() && p.1.is_finite()) || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (sx, sy) = ((rect[2] - rect[0]) / (x1 - x0), (rect[3] - rect[1]) / (y1 - y0));
    let result = [sx, 0.0, 0.0, sy, rect[0] - x0 * sx, rect[1] - y0 * sy];
    result.iter().all(|v| v.is_finite()).then_some(result)
}

/// Flatten comments and/or form fields on `pages`. Returns how many annotations were merged
/// into page content. Visible annotations without a usable appearance or placement refuse the
/// entire operation unchanged; dependent replies are removed only with a flattened parent.
pub fn flatten(doc: &mut Document, pages: &[usize], comments: bool, fields: bool) -> Result<usize, EditError> {
    let mut staged = doc.clone();
    let drawn = flatten_inner(&mut staged, pages, comments, fields)?;
    *doc = staged;
    Ok(drawn)
}

fn flatten_inner(doc: &mut Document, pages: &[usize], comments: bool, fields: bool) -> Result<usize, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut drawn = 0;
    let mut removed_widgets: HashSet<ObjRef> = HashSet::new();
    for &i in pages {
        let page = page_list(doc).get(i).cloned().ok_or(EditError::NoSuchPage(i))?;
        let annots_obj = page.dict.get(b"Annots").cloned();
        let Some(list) = annots_obj.as_ref().map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()) else { continue };
        let mut content = String::new();
        let mut res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let mut xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        let mut gone: HashSet<ObjRef> = HashSet::new();
        let mut gone_inline: HashSet<usize> = HashSet::new();
        let mut undrawn = Vec::new();
        for (k, entry) in list.iter().enumerate() {
            let obj = doc.resolve(entry);
            let Some(d) = obj.as_dict() else { continue };
            let subtype = d.name(b"Subtype").unwrap_or_default();
            let widget = subtype == b"Widget";
            if matches!(subtype, b"Link" | b"Popup") || (widget && !fields) || (!widget && !comments) {
                continue;
            }
            let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
            if flags & (HIDDEN | NO_VIEW) != 0 {
                continue;
            }
            let rect = nums(doc, d.get(b"Rect")).filter(|r| r.len() == 4).map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
            let placed = appearance(doc, d).zip(rect).and_then(|(ap, rect)| {
                let form = doc.get(ap);
                placement(doc, form.as_dict()?, rect).map(|m| (ap, m))
            });
            let Some((ap, m)) = placed else {
                // Replies can deliberately have no ink. Defer them until a successfully
                // flattened parent is known; every other selected visible entry must draw.
                undrawn.push(k);
                continue;
            };
            let name = crate::resource_names::reserve(&xobjects, &format!("PCFl{drawn}"), &Object::Ref(ap));
            let mut matrix = Vec::new();
            for v in m {
                pdfcraft_cos::serialize(&Object::Real(v), &mut matrix);
                matrix.push(b' ');
            }
            content.push_str(&format!("q {}cm /{name} Do Q\n", String::from_utf8_lossy(&matrix)));
            xobjects.set(name.into_bytes(), Object::Ref(ap));
            drawn += 1;
            match entry.as_ref() {
                Some(r) => {
                    gone.insert(r);
                    if widget {
                        removed_widgets.insert(r);
                    }
                }
                None => {
                    gone_inline.insert(k);
                }
            }
        }
        // Pop-ups and replies of what was flattened go too. Iterate to a fixpoint so
        // reply order does not matter; each pass either adds an entry or stops.
        loop {
            let before = gone.len() + gone_inline.len();
            for (k, entry) in list.iter().enumerate() {
                let obj = doc.resolve(entry);
                let Some(d) = obj.as_dict() else { continue };
                let points_at = |key: &[u8]| d.reference(key).is_some_and(|t| gone.contains(&t));
                if points_at(b"Parent") && d.name(b"Subtype") == Some(b"Popup") || points_at(b"IRT") {
                    match entry.as_ref() {
                        Some(r) => {
                            gone.insert(r);
                        }
                        None => {
                            gone_inline.insert(k);
                        }
                    }
                }
            }
            if gone.len() + gone_inline.len() == before {
                break;
            }
        }
        for k in undrawn {
            let removed = gone_inline.contains(&k) || list.get(k).and_then(Object::as_ref).is_some_and(|r| gone.contains(&r));
            if !removed {
                return Err(EditError::Invalid(format!(
                    "cannot flatten visible annotation {} on page {}: its normal appearance or placement is unusable; generate a usable appearance first",
                    k + 1,
                    i + 1
                )));
            }
        }
        if gone.is_empty() && gone_inline.is_empty() {
            continue;
        }
        let kept: Vec<Object> = list
            .iter()
            .enumerate()
            .filter(|(k, e)| !gone_inline.contains(k) && !e.as_ref().is_some_and(|r| gone.contains(&r)))
            .map(|(_, e)| e.clone())
            .collect();
        if !content.is_empty() {
            // The page's own resources, with the appearances as XObjects.
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
            doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
            let page = page_list(doc)[i].clone();
            place_tagged(doc, &page, "Flattened", content.into_bytes(), false)?;
        }
        let page_now = page_list(doc)[i].clone();
        match page_now.dict.get(b"Annots").and_then(|a| a.as_ref()).filter(|r| doc.get(*r).as_array().is_some()) {
            Some(r) => doc.set(r, Object::Array(kept)),
            None => doc.update_dict(page.obj, |d| {
                if kept.is_empty() {
                    d.remove(b"Annots");
                } else {
                    d.set(b"Annots".to_vec(), Object::Array(kept));
                }
            })?,
        }
    }
    if !removed_widgets.is_empty() {
        prune_fields(doc, &removed_widgets)?;
    }
    Ok(drawn)
}

/// Remove flattened widgets from the AcroForm field tree, and fields left without widgets.
fn prune_fields(doc: &mut Document, widgets: &HashSet<ObjRef>) -> Result<(), EditError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some(af) = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) else { return Ok(()) };
    let af_ref = af.as_ref();
    let mut af_dict = doc.resolve(&af).as_dict().cloned().unwrap_or_default();
    let fields = af_dict.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    // Returns whether the node survives.
    fn keep(doc: &mut Document, r: ObjRef, widgets: &HashSet<ObjRef>, depth: usize) -> bool {
        if widgets.contains(&r) || depth > 64 {
            return depth > 64;
        }
        let Some(d) = doc.get(r).as_dict().cloned() else { return true };
        let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) else { return true };
        let survivors: Vec<Object> = kids.iter().filter(|k| k.as_ref().is_none_or(|kr| keep(doc, kr, widgets, depth + 1))).cloned().collect();
        if survivors.len() != kids.len() {
            let _ = doc.update_dict(r, |d| d.set(b"Kids".to_vec(), Object::Array(survivors.clone())));
        }
        !survivors.is_empty()
    }
    let survivors: Vec<Object> = fields.iter().filter(|f| f.as_ref().is_none_or(|r| keep(doc, r, widgets, 0))).cloned().collect();
    af_dict.set(b"Fields".to_vec(), Object::Array(survivors));
    match af_ref {
        Some(r) => doc.set(r, Object::Dict(af_dict)),
        None => doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af_dict)))?,
    }
    Ok(())
}

#[cfg(test)]
mod preservation;
