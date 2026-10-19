use super::*;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use std::sync::Arc;

// Contributor-original imported border controls; standard Helvetica and vector paths only.
fn fixture(free_text: bool) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), num_array(&[0.0, 0.0, 360.0, 360.0]));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    for index in 0..2 {
        let rect = [20.0, 20.0 + index as f64 * 100.0, 200.0, 80.0 + index as f64 * 100.0];
        add_annotation(
            &mut doc,
            &NewAnnotation {
                page: 0,
                shape: if free_text && index == 0 { Shape::TextBox { rect, font_size: 12.0 } } else { Shape::Rectangle { rect } },
                style: Style { color: [0.1, 0.2, 0.3], opacity: 1.0, width: 2.0, fill: None },
                contents: "Ada".into(),
                author: "Original".into(),
            },
            &Meta::default(),
        )
        .unwrap();
    }
    let width = doc.add(Object::Real(2.0));
    let style = doc.add(Object::name("D"));
    let length = doc.add(Object::Real(5.5));
    let lengths = doc.add(Object::Array(vec![Object::Ref(length), Object::Int(2)]));
    let private = doc.add(PdfString::text("retained border metadata"));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Ref(width));
    bs.set(b"S".to_vec(), Object::Ref(style));
    bs.set(b"D".to_vec(), Object::Ref(lengths));
    bs.set(b"PrivateBorder".to_vec(), Object::Ref(private));
    let bs = doc.add(bs);
    for index in 0..2 {
        let r = annotation_ref(&doc, index);
        doc.update_dict(r, |d| d.set(b"BS".to_vec(), Object::Ref(bs))).unwrap();
        set_appearance(&mut doc, r).unwrap();
    }
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"OriginalBorder".to_vec(), Object::Ref(bs))).unwrap();
    saved(&doc)
}

fn saved(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn annotation_ref(doc: &Document, index: usize) -> ObjRef {
    annots(doc, page_ref(doc, 0).unwrap())[index].as_ref().unwrap()
}

fn original_border(doc: &Document) -> (ObjRef, Dict) {
    let root = doc.get(doc.root().unwrap());
    let r = root.as_dict().unwrap().reference(b"OriginalBorder").unwrap();
    (r, doc.get(r).as_dict().unwrap().clone())
}

fn normal(doc: &Document, index: usize) -> Stream {
    let d = annot_dict(doc, annotation_ref(doc, index));
    let ap = doc.resolve(d.get(b"AP").unwrap());
    let n = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = &*n else { panic!("normal appearance") };
    stream.clone()
}

fn assert_stroke(doc: &Document, width: f64) {
    let content = normal(doc, 0).decoded().unwrap();
    let parsed = pdfcraft_content::parse(&content);
    assert_eq!(parsed.skipped, 0);
    assert!(parsed.ops.iter().any(|o| o.is("w") && o.num(0) == Some(width)));
    assert!(parsed.ops.iter().any(|o| {
        o.is("d")
            && o.num(1) == Some(0.0)
            && o.operands
                .first()
                .and_then(Object::as_array)
                .is_some_and(|a| a.iter().map(Object::as_f64).collect::<Option<Vec<_>>>() == Some(vec![5.5, 2.0]))
    }));
    assert!(parsed.ops.iter().any(|o| o.is("S")));
    if annot_dict(doc, annotation_ref(doc, 0)).name(b"Subtype") == Some(b"FreeText") {
        assert!(parsed.ops.iter().any(|o| o.is("Tj")));
    }
}

#[test]
fn opacity_redraws_preserve_shared_indirect_annotation_border_operands_after_reopen() {
    for free_text in [false, true] {
        let mut doc = fixture(free_text);
        let (original, original_dict) = original_border(&doc);
        let other = annot_dict(&doc, annotation_ref(&doc, 1));
        let other_content = normal(&doc, 1).decoded().unwrap();
        if !free_text {
            assert_eq!(props(&doc, 0, 0).unwrap().width, Some(2.0));
        }
        set_style(&mut doc, 0, 0, None, Some(0.5), None, &Meta::default()).unwrap();
        assert_eq!(original_border(&doc), (original, original_dict));
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 1)), other);
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 0)).reference(b"BS"), Some(original));
        let doc = saved(&doc);
        let (original, bs) = original_border(&doc);
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 0)).reference(b"BS"), Some(original));
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 1)).reference(b"BS"), Some(original));
        assert_eq!(doc.resolve(bs.get(b"PrivateBorder").unwrap()).as_string().unwrap().to_text(), "retained border metadata");
        assert_eq!(normal(&doc, 1).decoded().unwrap(), other_content);
        assert_stroke(&doc, 2.0);
    }
}

#[test]
fn width_edits_copy_shared_indirect_annotation_borders_without_losing_style_or_private_data() {
    for free_text in [false, true] {
        let mut doc = fixture(free_text);
        let (original, original_dict) = original_border(&doc);
        let other = annot_dict(&doc, annotation_ref(&doc, 1));
        let other_content = normal(&doc, 1).decoded().unwrap();
        set_style(&mut doc, 0, 0, None, None, Some(3.25), &Meta::default()).unwrap();
        assert_eq!(original_border(&doc), (original, original_dict));
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 1)), other);
        let doc = saved(&doc);
        let (original, bs) = original_border(&doc);
        let target = annot_dict(&doc, annotation_ref(&doc, 0));
        let edited = target.get(b"BS").unwrap().as_dict().unwrap();
        let mut expected = bs.clone();
        expected.set(b"W".to_vec(), Object::Real(3.25));
        assert_eq!(edited, &expected);
        assert_eq!(doc.resolve(bs.get(b"W").unwrap()).as_f64(), Some(2.0));
        assert_eq!(annot_dict(&doc, annotation_ref(&doc, 1)).reference(b"BS"), Some(original));
        assert_eq!(normal(&doc, 1).decoded().unwrap(), other_content);
        assert_stroke(&doc, 3.25);
    }
}
