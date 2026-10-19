//! Contributor-original ordinary annotations and a field; no external fixtures or assets.
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, write_full};

use super::extract_pages;

fn stream(dict: &str, content: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

fn fixture() -> Document {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 14 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] /Resources 11 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots 15 0 R >>".into(),
        stream("", "BT /F 12 Tf 20 260 Td (Shared page) Tj ET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 50 20] /Resources << /Font << /F 9 0 R >> >>", "BT /F 12 Tf 0 2 Td (Value) Tj ET"),
        "<< /Type /Annot /Subtype /Text /Rect [20 180 70 200] /P 3 0 R /Contents (Parent) /Popup 7 0 R /AP << /N 5 0 R >> /Custom (preserved) >>"
            .into(),
        "<< /Type /Annot /Subtype /Popup /Rect [80 150 180 220] /P 3 0 R /Parent 6 0 R >>".into(),
        "<< /Type /Annot /Subtype /Text /Rect [20 140 70 160] /P 3 0 R /Contents (Reply) /IRT 6 0 R /AP << /N 5 0 R >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [20 80 120 100] /P 3 0 R /A << /S /URI /URI (https://example.invalid/original) >> >>".into(),
        "<< /Font << /F 9 0 R >> >>".into(),
        "<< /FT /Tx /T (entry) /V (Value) /Kids [13 0 R] /DA (/F 12 Tf 0 g) >>".into(),
        "<< /Type /Annot /Subtype /Widget /Rect [20 30 70 50] /P 3 0 R /Parent 12 0 R /AP << /N 5 0 R >> >>".into(),
        "<< /Fields [12 0 R] /DR << /Font << /F 9 0 R >> >> >>".into(),
        // Replies and popups deliberately precede their parent; Annots itself is indirect.
        "[8 0 R 7 0 R 6 0 R 10 0 R 13 0 R]".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

fn reopen(doc: &Document) -> Document {
    Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap()
}

fn page_dicts(doc: &Document) -> Vec<(ObjRef, Dict)> {
    crate::walk(doc).unwrap().into_iter().map(|(r, _)| (r, doc.get(r).as_dict().unwrap().clone())).collect()
}

fn annotations(doc: &Document, page: &Dict) -> Vec<ObjRef> {
    doc.resolve(page.get(b"Annots").unwrap()).as_array().unwrap().iter().map(|o| o.as_ref().unwrap()).collect()
}

fn dict(doc: &Document, r: ObjRef) -> Dict {
    doc.get(r).as_dict().unwrap().clone()
}

fn normal_appearance(doc: &Document, r: ObjRef) -> ObjRef {
    doc.resolve(dict(doc, r).get(b"AP").unwrap()).as_dict().unwrap().reference(b"N").unwrap()
}

#[test]
fn repeated_pages_keep_local_annotation_relationships_and_shared_appearances() {
    let src = fixture();
    let source_bytes = write_full(&src, &SaveOptions::default()).unwrap();
    let mut doc = reopen(&extract_pages(&src, &[0, 0]).unwrap());
    let pages = page_dicts(&doc);
    assert_eq!(pages.len(), 2);
    let first = annotations(&doc, &pages[0].1);
    let second = annotations(&doc, &pages[1].1);
    assert_eq!(first.len(), 5);
    assert_eq!(second.len(), 5);
    assert!(first.iter().all(|r| !second.contains(r)), "each copied page owns its annotations");
    for (page, copied) in pages.iter().zip([&first, &second]) {
        for &r in copied {
            assert_eq!(dict(&doc, r).reference(b"P"), Some(page.0));
        }
        assert_eq!(dict(&doc, copied[0]).reference(b"IRT"), Some(copied[2]));
        assert_eq!(dict(&doc, copied[1]).reference(b"Parent"), Some(copied[2]));
        assert_eq!(dict(&doc, copied[2]).reference(b"Popup"), Some(copied[1]));
        assert_eq!(dict(&doc, copied[2]).get(b"Custom").unwrap().as_string().unwrap().bytes, b"preserved");
        let action = doc.resolve(dict(&doc, copied[3]).get(b"A").unwrap());
        assert_eq!(action.as_dict().unwrap().get(b"URI").unwrap().as_string().unwrap().bytes, b"https://example.invalid/original");
    }
    assert_eq!(normal_appearance(&doc, first[2]), normal_appearance(&doc, second[2]), "immutable appearances remain shared");
    let fonts: Vec<_> = pages
        .iter()
        .map(|(_, p)| {
            let resources = doc.resolve(p.get(b"Resources").unwrap());
            doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().reference(b"F").unwrap()
        })
        .collect();
    assert_eq!(fonts[0], fonts[1], "immutable font resources remain shared");
    assert_eq!(pages[0].1.reference(b"Contents"), pages[1].1.reference(b"Contents"));
    // Editing one copy and then removing its comment/reply/popup leaves the other intact.
    doc.update_dict(first[2], |d| d.set(b"Contents".to_vec(), Object::String(PdfString::literal(b"Changed first".to_vec())))).unwrap();
    let edited = reopen(&doc);
    let edited_pages = page_dicts(&edited);
    let edited_first = annotations(&edited, &edited_pages[0].1);
    assert_eq!(dict(&edited, edited_first[2]).get(b"Contents").unwrap().as_string().unwrap().bytes, b"Changed first");
    let edited_second = annotations(&edited, &edited_pages[1].1);
    assert_eq!(dict(&edited, edited_second[2]).get(b"Contents").unwrap().as_string().unwrap().bytes, b"Parent");
    let kept: Vec<_> = first.iter().skip(3).copied().map(Object::Ref).collect();
    doc.update_dict(pages[0].0, |d| d.set(b"Annots".to_vec(), Object::Array(kept))).unwrap();
    let saved = reopen(&doc);
    let saved_pages = page_dicts(&saved);
    assert_eq!(annotations(&saved, &saved_pages[0].1).len(), 2);
    let intact = annotations(&saved, &saved_pages[1].1);
    assert_eq!(intact.len(), 5);
    assert_eq!(dict(&saved, intact[2]).get(b"Contents").unwrap().as_string().unwrap().bytes, b"Parent");
    assert_eq!(dict(&saved, intact[0]).reference(b"IRT"), Some(intact[2]));
    assert_eq!(dict(&saved, intact[1]).reference(b"Parent"), Some(intact[2]));
    assert_eq!(dict(&saved, intact[2]).reference(b"Popup"), Some(intact[1]));
    assert_eq!(write_full(&src, &SaveOptions::default()).unwrap(), source_bytes, "source document remains unchanged");
}

#[test]
fn repeated_page_widgets_keep_both_kids_under_one_shared_field() {
    let src = fixture();
    let single = reopen(&extract_pages(&src, &[0]).unwrap());
    let single_widget = annotations(&single, &page_dicts(&single)[0].1)[4];
    let single_parent = dict(&single, single_widget).reference(b"Parent").unwrap();
    assert_eq!(single.resolve(dict(&single, single_parent).get(b"Kids").unwrap()).as_array().unwrap(), &[Object::Ref(single_widget)]);
    let doc = reopen(&extract_pages(&src, &[0, 0]).unwrap());
    let pages = page_dicts(&doc);
    let widgets: Vec<_> = pages.iter().map(|(_, p)| annotations(&doc, p)[4]).collect();
    assert_ne!(widgets[0], widgets[1]);
    let parents: Vec<_> = widgets.iter().map(|r| dict(&doc, *r).reference(b"Parent").unwrap()).collect();
    assert_eq!(parents[0], parents[1], "copies represent the same logical field");
    let parent = dict(&doc, parents[0]);
    let kids = doc.resolve(parent.get(b"Kids").unwrap()).as_array().unwrap().clone();
    assert_eq!(kids.len(), 2);
    assert!(widgets.iter().all(|r| kids.contains(&Object::Ref(*r))));
    let catalog = doc.get(doc.root().unwrap());
    let form = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(form.as_dict().unwrap().get(b"Fields").unwrap());
    assert_eq!(fields.as_array().unwrap(), &[Object::Ref(parents[0])]);
    assert_eq!(parent.get(b"V").unwrap().as_string().unwrap().bytes, b"Value");
}

#[test]
fn cross_page_replies_keep_first_target_when_page_is_repeated() {
    for target_has_p in [true, false] {
        let mut src = fixture();
        if !target_has_p {
            src.update_dict(ObjRef::new(6, 0), |d| {
                d.remove(b"P");
            })
            .unwrap();
        }
        let other_page = src.add(Object::Null);
        let mut reply = dict(&src, ObjRef::new(8, 0));
        reply.set(b"P".to_vec(), Object::Ref(other_page));
        let reply = src.add(Object::Dict(reply));
        let mut page = dict(&src, ObjRef::new(3, 0));
        page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(reply)]));
        src.set(other_page, Object::Dict(page));
        src.update_dict(ObjRef::new(2, 0), |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(other_page)]));
            d.set(b"Count".to_vec(), Object::Int(2));
        })
        .unwrap();
        // Repeat either the target page or the replying page, with and without target /P.
        for requested in [[0, 0, 1], [0, 1, 1]] {
            let copied = reopen(&extract_pages(&src, &requested).unwrap());
            let pages = page_dicts(&copied);
            assert_eq!(pages.len(), 3);
            let first = annotations(&copied, &pages[0].1);
            assert_eq!(dict(&copied, first[2]).reference(b"P"), Some(pages[0].0));
            let second = annotations(&copied, &pages[1].1);
            let third = annotations(&copied, &pages[2].1);
            if requested[1] == 0 {
                assert_ne!(first[2], second[2]);
            } else {
                assert_ne!(second[0], third[0]);
            }
            for (i, &source_page) in requested.iter().enumerate() {
                if source_page == 1 {
                    let reply = annotations(&copied, &pages[i].1)[0];
                    let target = dict(&copied, reply).reference(b"IRT").unwrap();
                    assert_eq!(target, first[2], "cross-page reply keeps the actual first target, /P={target_has_p}");
                    assert!(first.contains(&target), "reply target is listed on its owning page");
                    assert_eq!(dict(&copied, reply).reference(b"P"), Some(pages[i].0));
                }
            }
        }
    }
}
