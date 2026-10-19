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

#[test]
fn repeated_merged_widgets_keep_one_root_or_nested_field_and_fill_all_copies() {
    use pdfcraft_forms::{Field, FieldEvent, FieldValue, ScriptResult, Scripts};
    #[derive(Default)]
    struct Events(Vec<FieldEvent>);
    impl Scripts for Events {
        fn run(&mut self, event: FieldEvent, _: &str, _: &Field, value: &str, _: &[Field]) -> ScriptResult {
            self.0.push(event);
            ScriptResult { rc: true, value: value.to_string(), changes: Vec::new(), message: None }
        }
    }
    for nested in [false, true] {
        let mut src = fixture();
        // Source field 13 is merged with its widget, either root-level or below named field 12.
        let mut aa = Dict::new();
        for (key, js) in
            [(b"K".as_slice(), "event.rc = true;"), (b"F".as_slice(), "event.value = event.value;"), (b"E".as_slice(), "event.target.display = 0;")]
        {
            let mut action = Dict::new();
            action.set(b"S".to_vec(), Object::name("JavaScript"));
            action.set(b"JS".to_vec(), PdfString::literal(js.as_bytes().to_vec()));
            aa.set(key.to_vec(), Object::Ref(src.add(Object::Dict(action))));
        }
        aa.set(b"OriginalUnknown".to_vec(), PdfString::literal(b"retained".to_vec()));
        let aa = src.add(Object::Dict(aa));
        let mut action = Dict::new();
        action.set(b"S".to_vec(), Object::name("URI"));
        action.set(b"URI".to_vec(), PdfString::literal(b"https://example.invalid/widget".to_vec()));
        let action = src.add(Object::Dict(action));
        src.update_dict(ObjRef::new(13, 0), |d| {
            d.set(b"F".to_vec(), Object::Int(4));
            d.set(b"FT".to_vec(), Object::name("Tx"));
            d.set(b"T".to_vec(), PdfString::literal(b"entry".to_vec()));
            d.set(b"V".to_vec(), PdfString::literal(b"Value".to_vec()));
            d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
            d.set(b"AA".to_vec(), Object::Ref(aa));
            d.set(b"A".to_vec(), Object::Ref(action));
            d.set(b"OriginalUnknown".to_vec(), PdfString::literal(b"widget metadata".to_vec()));
            if !nested {
                d.remove(b"Parent");
            } else {
                // An explicit empty Kids array still describes a terminal merged widget.
                d.set(b"Kids".to_vec(), Object::Array(Vec::new()));
            }
        })
        .unwrap();
        if nested {
            src.update_dict(ObjRef::new(12, 0), |d| {
                d.set(b"T".to_vec(), PdfString::literal(b"group".to_vec()));
            })
            .unwrap();
        } else {
            src.update_dict(ObjRef::new(14, 0), |d| {
                d.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(13, 0))]));
            })
            .unwrap();
        }
        let name = if nested { "group.entry" } else { "entry" };
        let original = write_full(&src, &SaveOptions::default()).unwrap();
        let single = reopen(&extract_pages(&src, &[0]).unwrap());
        let fields = pdfcraft_forms::fields(&single);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, name);
        assert_eq!(fields[0].widgets.len(), 1);
        assert_eq!(fields[0].obj, fields[0].widgets[0].obj, "single imports retain the merged representation");
        let mut copied = reopen(&extract_pages(&src, &[0, 0]).unwrap());
        let pages = page_dicts(&copied);
        let fields = pdfcraft_forms::fields(&copied);
        assert_eq!(fields.len(), 1, "one original field, not duplicate qualified names");
        assert_eq!(fields[0].name, name);
        assert_eq!(fields[0].value, vec!["Value"]);
        assert_eq!(fields[0].widgets.len(), 2);
        let field = dict(&copied, fields[0].obj);
        assert!(!field.contains(b"Rect"));
        assert!(!field.contains(b"A"));
        assert!(!field.contains(b"F"), "annotation flags belong to the widget");
        let field_aa = copied.resolve(field.get(b"AA").unwrap());
        let field_aa = field_aa.as_dict().unwrap();
        assert!(field_aa.contains(b"K"));
        assert!(field_aa.contains(b"F"), "Format action belongs to the field");
        assert!(!field_aa.contains(b"E"));
        assert_eq!(field_aa.get(b"OriginalUnknown").unwrap().as_string().unwrap().bytes, b"retained");
        let kid_refs = copied.resolve(field.get(b"Kids").unwrap()).as_array().unwrap().clone();
        for (index, widget) in fields[0].widgets.iter().enumerate() {
            assert_ne!(widget.obj, fields[0].obj);
            assert_eq!(widget.page, Some(index));
            assert!(kid_refs.contains(&Object::Ref(widget.obj)));
            assert!(annotations(&copied, &pages[index].1).contains(&widget.obj));
            let d = dict(&copied, widget.obj);
            assert_eq!(d.reference(b"Parent"), Some(fields[0].obj));
            assert_eq!(d.reference(b"P"), Some(pages[index].0));
            assert!(!d.contains(b"T"));
            assert!(!d.contains(b"FT"));
            assert!(!d.contains(b"V"));
            assert_eq!(d.int(b"F"), Some(4));
            assert_eq!(d.get(b"OriginalUnknown").unwrap().as_string().unwrap().bytes, b"widget metadata");
            let widget_aa = copied.resolve(d.get(b"AA").unwrap());
            let widget_aa = widget_aa.as_dict().unwrap();
            assert!(widget_aa.contains(b"E"));
            assert!(!widget_aa.contains(b"K"));
            assert!(!widget_aa.contains(b"F"));
            assert_eq!(widget_aa.get(b"OriginalUnknown").unwrap().as_string().unwrap().bytes, b"retained");
            let a = copied.resolve(d.get(b"A").unwrap());
            assert_eq!(a.as_dict().unwrap().get(b"URI").unwrap().as_string().unwrap().bytes, b"https://example.invalid/widget");
        }
        assert_eq!(kid_refs.len(), 2);
        assert_eq!(normal_appearance(&copied, fields[0].widgets[0].obj), normal_appearance(&copied, fields[0].widgets[1].obj));
        if nested {
            let parent = field.reference(b"Parent").unwrap();
            let group = dict(&copied, parent);
            assert_eq!(group.get(b"T").unwrap().as_string().unwrap().bytes, b"group");
            assert_eq!(copied.resolve(group.get(b"Kids").unwrap()).as_array().unwrap(), &[Object::Ref(fields[0].obj)]);
        } else {
            assert!(!field.contains(b"Parent"));
        }
        pdfcraft_forms::set_value(&mut copied, name, &FieldValue::Text("Changed".into())).unwrap();
        let saved = reopen(&copied);
        let fields = pdfcraft_forms::fields(&saved);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, vec!["Changed"]);
        assert_eq!(fields[0].widgets.len(), 2);
        for widget in &fields[0].widgets {
            let ap = saved.get(normal_appearance(&saved, widget.obj));
            let Object::Stream(stream) = &*ap else { panic!("expected widget appearance stream") };
            let bytes = stream.decoded().unwrap();
            assert!(bytes.windows(b"Changed".len()).any(|b| b == b"Changed"), "both appearances display the updated shared value");
        }
        let mut events = Events::default();
        pdfcraft_forms::set_value_with(&mut copied, name, &FieldValue::Text("Again".into()), &mut events).unwrap();
        assert_eq!(events.0, vec![FieldEvent::Keystroke, FieldEvent::Format], "field events run once per logical update");
        assert_eq!(write_full(&src, &SaveOptions::default()).unwrap(), original, "source bytes remain unchanged");
    }
}

#[test]
fn repeated_page_imports_keep_local_graphs_through_incremental_saves() {
    fn incremental(doc: &Document) -> Document {
        assert!(!doc.revisions().is_empty() && !doc.encryption_changed() && !doc.full_save_required());
        let bytes = pdfcraft_cos::write_incremental(doc, &SaveOptions::default()).unwrap();
        assert!(bytes.len() > doc.bytes().len());
        assert!(bytes.starts_with(doc.bytes().as_slice()), "each incremental update retains its entire parsed baseline");
        let independent = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
        assert_eq!(independent.pages().len(), 3);
        Document::open(Arc::new(bytes)).unwrap()
    }

    fn graph(doc: &Document) -> Vec<Vec<ObjRef>> {
        let pages = page_dicts(doc);
        assert_eq!(pages.len(), 3);
        let copies: Vec<_> = pages.iter().map(|(_, page)| annotations(doc, page)).collect();
        for (index, ((page, _), copied)) in pages.iter().zip(&copies).enumerate() {
            assert_eq!(copied.len(), 5);
            assert!(copies.iter().skip(index + 1).all(|other| copied.iter().all(|r| !other.contains(r))));
            for &r in copied {
                assert_eq!(dict(doc, r).reference(b"P"), Some(*page));
            }
            assert_eq!(dict(doc, copied[0]).reference(b"IRT"), Some(copied[2]));
            assert_eq!(dict(doc, copied[1]).reference(b"Parent"), Some(copied[2]));
            assert_eq!(dict(doc, copied[2]).reference(b"Popup"), Some(copied[1]));
        }
        let parent = dict(doc, copies[1][4]).reference(b"Parent").unwrap();
        assert_eq!(dict(doc, copies[2][4]).reference(b"Parent"), Some(parent));
        assert_ne!(dict(doc, copies[0][4]).reference(b"Parent"), Some(parent));
        let kids = doc.resolve(dict(doc, parent).get(b"Kids").unwrap()).as_array().unwrap().clone();
        assert_eq!(kids.len(), 2);
        assert!(kids.contains(&Object::Ref(copies[1][4])) && kids.contains(&Object::Ref(copies[2][4])));
        copies
    }

    let src = fixture();
    let source_bytes = write_full(&src, &SaveOptions::default()).unwrap();
    let mut destination = fixture();
    destination.update_dict(ObjRef::new(12, 0), |d| d.set(b"T".to_vec(), PdfString::literal(b"destination".to_vec()))).unwrap();
    // Parse an existing destination with its own annotation graph and distinct logical field.
    let mut destination = reopen(&destination);
    let original_page = page_dicts(&destination)[0].clone();
    let original_annots = annotations(&destination, &original_page.1);
    let original_dicts: Vec<_> = original_annots.iter().map(|r| dict(&destination, *r)).collect();
    let inserted = super::import_pages(&mut destination, &src, &[0, 0], 1).unwrap();
    assert_eq!(inserted.len(), 2);
    assert_ne!(inserted[0], inserted[1]);
    let mut destination = incremental(&destination);
    let copies = graph(&destination);
    assert_eq!(page_dicts(&destination)[0].0, original_page.0);
    assert_eq!(copies[0], original_annots);
    for (&r, before) in copies[0].iter().zip(&original_dicts) {
        assert_eq!(&dict(&destination, r), before, "pre-existing annotations are untouched");
    }
    assert_eq!(normal_appearance(&destination, copies[1][2]), normal_appearance(&destination, copies[2][2]));
    let fields = pdfcraft_forms::fields(&destination);
    assert_eq!(fields.len(), 2);
    let imported = fields.iter().find(|f| f.name == "entry").unwrap();
    assert_eq!(imported.widgets.len(), 2);
    assert!(imported.widgets.iter().all(|w| copies[1][4] == w.obj || copies[2][4] == w.obj));
    let baseline = destination.bytes().clone();
    destination.update_dict(copies[1][2], |d| d.set(b"Contents".to_vec(), PdfString::literal(b"Changed first".to_vec()))).unwrap();
    pdfcraft_forms::set_value(&mut destination, "entry", &pdfcraft_forms::FieldValue::Text("Updated imported".into())).unwrap();
    let destination = incremental(&destination);
    assert!(destination.bytes().starts_with(baseline.as_slice()));
    let copies = graph(&destination);
    assert_eq!(dict(&destination, copies[1][2]).get(b"Contents").unwrap().as_string().unwrap().bytes, b"Changed first");
    assert_eq!(dict(&destination, copies[2][2]).get(b"Contents").unwrap().as_string().unwrap().bytes, b"Parent");
    for (&r, before) in copies[0].iter().zip(&original_dicts) {
        assert_eq!(&dict(&destination, r), before, "later edits leave the existing destination graph unchanged");
    }
    let fields = pdfcraft_forms::fields(&destination);
    assert_eq!(fields.len(), 2);
    let imported = fields.iter().find(|f| f.name == "entry").unwrap();
    assert_eq!(imported.value, ["Updated imported"]);
    assert_eq!(imported.widgets.len(), 2);
    for widget in &imported.widgets {
        let object = destination.get(normal_appearance(&destination, widget.obj));
        let Object::Stream(stream) = &*object else { panic!("expected imported widget appearance") };
        assert!(stream.decoded().unwrap().windows(b"Updated imported".len()).any(|b| b == b"Updated imported"));
    }
    let existing = fields.iter().find(|f| f.name == "destination").unwrap();
    assert_eq!(existing.value, ["Value"]);
    assert_eq!(existing.widgets.len(), 1);
    assert_eq!(write_full(&src, &SaveOptions::default()).unwrap(), source_bytes, "source document remains unchanged");
}
