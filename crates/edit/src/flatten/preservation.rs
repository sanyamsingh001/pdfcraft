//! Contributor-original ordinary imported annotation and widget fixtures; no external assets.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_full, write_incremental};

use super::flatten;

fn array(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

fn fixture() -> Document {
    let mut doc = Document::new_empty();
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut kids = Vec::new();
    for _ in 0..2 {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), array(&[0.0, 0.0, 600.0, 800.0]));
        page.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        kids.push(Object::Ref(doc.add(page)));
    }
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc
}

fn appearance(doc: &mut Document, bbox: &[f64], matrix: Option<&[f64]>, bytes: &[u8]) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), array(bbox));
    if let Some(matrix) = matrix {
        d.set(b"Matrix".to_vec(), array(matrix));
    }
    d.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
    doc.add(Object::Stream(Stream::from_raw(d, bytes.to_vec())))
}

fn annotation(doc: &mut Document, page: usize, subtype: &str, ap: Option<ObjRef>, rect: &[f64]) -> ObjRef {
    let p = pdfcraft_model::pages(doc)[page].obj;
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"P".to_vec(), Object::Ref(p));
    d.set(b"Rect".to_vec(), array(rect));
    d.set(b"Contents".to_vec(), PdfString::text("Keep the original comment"));
    if subtype == "Text" {
        d.set(b"Name".to_vec(), Object::name("Comment"));
    }
    if let Some(ap) = ap {
        let mut appearances = Dict::new();
        appearances.set(b"N".to_vec(), Object::Ref(ap));
        d.set(b"AP".to_vec(), Object::Dict(appearances));
    }
    let r = doc.add(d);
    let mut annots = doc.get(p).as_dict().unwrap().get(b"Annots").map(|o| doc.resolve(o).as_array().unwrap().clone()).unwrap_or_default();
    annots.push(Object::Ref(r));
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
    r
}

fn saved(doc: &Document) -> Document {
    Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap()
}

fn assert_refused_unchanged(doc: &mut Document, pages: &[usize], comments: bool, fields: bool) {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let objects: Vec<_> = doc.object_numbers().into_iter().map(|n| (n, doc.get(ObjRef::new(n, doc.generation(n))))).collect();
    let changed = doc.modified_objects();
    let error = flatten(doc, pages, comments, fields).unwrap_err().to_string();
    assert!(error.contains("normal appearance or placement is unusable"), "{error}");
    assert_eq!(write_incremental(doc, &SaveOptions::default()).unwrap(), bytes);
    assert_eq!(doc.modified_objects(), changed);
    assert_eq!(doc.object_numbers(), objects.iter().map(|(n, _)| *n).collect::<Vec<_>>());
    for (n, object) in objects {
        assert_eq!(doc.get(ObjRef::new(n, doc.generation(n))), object);
    }
}

#[test]
fn visible_notes_without_appearances_refuse_the_entire_multi_page_operation() {
    let mut doc = fixture();
    let ap = appearance(&mut doc, &[0.0, 0.0, 10.0, 10.0], None, b"0 0 10 10 re f");
    annotation(&mut doc, 0, "Square", Some(ap), &[20.0, 20.0, 40.0, 40.0]);
    annotation(&mut doc, 1, "Text", None, &[100.0, 100.0, 120.0, 120.0]);
    let mut doc = saved(&doc);
    assert_refused_unchanged(&mut doc, &[0, 1], true, false);
    // Excluding comments must not inspect their missing appearances.
    assert_eq!(flatten(&mut doc, &[0, 1], false, true).unwrap(), 0);
}

#[test]
fn need_appearances_widgets_without_ap_keep_values_and_the_field_tree_on_refusal() {
    let mut doc = fixture();
    let r = annotation(&mut doc, 0, "Widget", None, &[20.0, 20.0, 200.0, 50.0]);
    doc.update_dict(r, |d| {
        d.set(b"FT".to_vec(), Object::name("Tx"));
        d.set(b"T".to_vec(), PdfString::text("OriginalName"));
        d.set(b"V".to_vec(), PdfString::text("Ada"));
        d.set(b"DA".to_vec(), PdfString::text("/Helv 12 Tf 0 g"));
    })
    .unwrap();
    let mut af = Dict::new();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let mut fonts = Dict::new();
    fonts.set(b"Helv".to_vec(), Object::Dict(font));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    af.set(b"DR".to_vec(), Object::Dict(resources));
    af.set(b"NeedAppearances".to_vec(), Object::Bool(true));
    af.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(r)]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af))).unwrap();
    let mut doc = saved(&doc);
    assert_refused_unchanged(&mut doc, &[0], false, true);
    assert_eq!(flatten(&mut doc, &[0], true, false).unwrap(), 0);
}

#[test]
fn unusable_ordinary_geometry_refuses_without_deleting_or_pruning_annotations() {
    for (bbox, matrix, rect) in [
        (vec![0.0, 0.0, 0.0, 10.0], None, vec![20.0, 20.0, 40.0, 40.0]),
        (vec![0.0, 0.0, 10.0, 10.0], Some(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0]), vec![20.0, 20.0, 40.0, 40.0]),
        (vec![0.0, 0.0, 10.0, 10.0], None, vec![20.0, 20.0, 20.0, 40.0]),
        (vec![0.0, 0.0, 10.0, 10.0], Some(vec![1.0, 0.0]), vec![20.0, 20.0, 40.0, 40.0]),
    ] {
        let mut doc = fixture();
        let ap = appearance(&mut doc, &bbox, matrix.as_deref(), b"0 0 10 10 re f");
        annotation(&mut doc, 0, "Square", Some(ap), &rect);
        let mut doc = saved(&doc);
        assert_refused_unchanged(&mut doc, &[0], true, false);
    }
}

#[test]
fn successful_flatten_keeps_hidden_notes_and_draws_blank_state_appearances_and_removes_dependent_replies() {
    let mut doc = fixture();
    let ap = appearance(&mut doc, &[0.0, 0.0, 10.0, 10.0], None, b"0 0 10 10 re f");
    let parent = annotation(&mut doc, 0, "Square", Some(ap), &[20.0, 20.0, 40.0, 40.0]);
    let empty = appearance(&mut doc, &[0.0; 4], None, b"");
    let reply = annotation(&mut doc, 0, "Text", Some(empty), &[20.0, 20.0, 40.0, 40.0]);
    doc.update_dict(reply, |d| d.set(b"IRT".to_vec(), Object::Ref(parent))).unwrap();
    let nested = annotation(&mut doc, 0, "Text", None, &[20.0, 20.0, 40.0, 40.0]);
    doc.update_dict(nested, |d| d.set(b"IRT".to_vec(), Object::Ref(reply))).unwrap();
    let hidden = annotation(&mut doc, 0, "Text", None, &[100.0, 100.0, 120.0, 120.0]);
    doc.update_dict(hidden, |d| d.set(b"F".to_vec(), Object::Int(2))).unwrap();
    let no_view = annotation(&mut doc, 0, "Text", None, &[100.0, 100.0, 120.0, 120.0]);
    doc.update_dict(no_view, |d| d.set(b"F".to_vec(), Object::Int(32))).unwrap();
    annotation(&mut doc, 0, "Link", None, &[100.0, 300.0, 200.0, 320.0]);
    let popup = annotation(&mut doc, 0, "Popup", None, &[100.0, 400.0, 200.0, 450.0]);
    doc.update_dict(popup, |d| d.set(b"Parent".to_vec(), Object::Ref(parent))).unwrap();
    // Reverse reply order: dependency closure must not rely on annotation ordering.
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        let annots = d.get(b"Annots").unwrap().as_array().unwrap();
        let mut annots = annots.clone();
        annots.swap(1, 2);
        d.set(b"Annots".to_vec(), Object::Array(annots));
    })
    .unwrap();
    let blank = appearance(&mut doc, &[0.0, 0.0, 10.0, 10.0], None, b"");
    doc.update_dict(blank, |d| d.set(b"Matrix".to_vec(), Object::Null)).unwrap();
    let widget = annotation(&mut doc, 0, "Widget", None, &[100.0, 200.0, 120.0, 220.0]);
    let state = doc.add(Object::name("Off"));
    let mut normal = Dict::new();
    normal.set(b"Off".to_vec(), Object::Ref(blank));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Dict(normal));
    doc.update_dict(widget, |d| {
        d.set(b"FT".to_vec(), Object::name("Btn"));
        d.set(b"T".to_vec(), PdfString::text("Check"));
        d.set(b"AS".to_vec(), Object::Ref(state));
        d.set(b"AP".to_vec(), Object::Dict(ap));
    })
    .unwrap();
    // An unselected page's field does not need an appearance and must stay in the tree.
    let retained = annotation(&mut doc, 1, "Widget", None, &[20.0, 20.0, 200.0, 50.0]);
    doc.update_dict(retained, |d| {
        d.set(b"FT".to_vec(), Object::name("Tx"));
        d.set(b"T".to_vec(), PdfString::text("OtherPage"));
        d.set(b"V".to_vec(), PdfString::text("Retained value"));
    })
    .unwrap();
    let mut af = Dict::new();
    af.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(widget), Object::Ref(retained)]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af))).unwrap();
    let mut doc = saved(&doc);
    assert_eq!(flatten(&mut doc, &[0, 0], true, true).unwrap(), 2);
    let doc = saved(&doc);
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let annots = doc.resolve(page.dict.get(b"Annots").unwrap());
    let kept = annots.as_array().unwrap();
    assert_eq!(kept.len(), 3);
    assert_eq!(doc.resolve(&kept[0]).as_dict().unwrap().int(b"F"), Some(2));
    assert_eq!(doc.resolve(&kept[1]).as_dict().unwrap().int(b"F"), Some(32));
    assert_eq!(doc.resolve(&kept[2]).as_dict().unwrap().name(b"Subtype"), Some(b"Link".as_slice()));
    let catalog = doc.get(doc.root().unwrap());
    let af = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(af.as_dict().unwrap().get(b"Fields").unwrap());
    let fields = fields.as_array().unwrap();
    assert_eq!(fields.len(), 1);
    let retained = doc.resolve(&fields[0]);
    assert_eq!(retained.as_dict().unwrap().get(b"V"), Some(&Object::String(PdfString::literal(b"Retained value".to_vec()))));
    let other = pdfcraft_model::pages(&doc)[1].clone();
    let other_annots = doc.resolve(other.dict.get(b"Annots").unwrap());
    assert_eq!(other_annots.as_array().unwrap(), fields);
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let xo = xo.as_dict().unwrap();
    assert_eq!(xo.len(), 2);
    let first = doc.resolve(xo.get(b"PCFl0").unwrap());
    let Object::Stream(first) = &*first else { panic!() };
    assert_eq!(first.decoded().unwrap(), b"0 0 10 10 re f");
    let blank = doc.resolve(xo.get(b"PCFl1").unwrap());
    let Object::Stream(blank) = &*blank else { panic!() };
    assert!(blank.decoded().unwrap().is_empty());
    let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
    let entries = contents.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let contents = doc.resolve(&entries[0]);
    let Object::Stream(contents) = &*contents else { panic!() };
    let bytes = contents.decoded().unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("q 2 0 0 2 20 20 cm /PCFl0 Do Q"));
    assert!(String::from_utf8_lossy(&bytes).contains("/PCFl1 Do"));
}
