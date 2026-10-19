use super::*;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use std::sync::Arc;

// Contributor-original field borders, blank page, vector appearances and standard technical fonts.
fn blank() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 600, 800].into_iter().map(Object::Int).collect()));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

fn saved(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn field(doc: &Document) -> Field {
    fields(doc).into_iter().find(|f| f.name == "Border").unwrap()
}

fn normal_streams(doc: &Document, w: &Widget) -> Vec<Stream> {
    let widget = doc.get(w.obj);
    let ap = doc.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
    let n = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let objects = match &*n {
        Object::Stream(_) => vec![(*n).clone()],
        Object::Dict(d) => d.iter().map(|(_, o)| (*doc.resolve(o)).clone()).collect(),
        _ => panic!("normal appearances"),
    };
    objects
        .into_iter()
        .map(|o| match o {
            Object::Stream(s) => s,
            _ => panic!("appearance stream"),
        })
        .collect()
}

fn original(kind: NewField, dash: Option<Vec<f64>>) -> Document {
    let mut doc = blank();
    add_field(&mut doc, 0, [20.0, 20.0, 180.0, 60.0], &kind, Some("Border")).unwrap();
    let f = field(&doc);
    let w = &f.widgets[0];
    let mut mk = Dict::new();
    let black = doc.add(Object::Real(0.0));
    let white = doc.add(Object::Real(1.0));
    let border = doc.add(Object::Array(vec![Object::Ref(black); 3]));
    let fill = doc.add(Object::Array(vec![Object::Ref(white); 3]));
    mk.set(b"BC".to_vec(), Object::Ref(border));
    mk.set(b"BG".to_vec(), Object::Ref(fill));
    if matches!(kind, NewField::Button { .. }) {
        mk.set(b"CA".to_vec(), PdfString::text("Submit"));
    }
    mk.set(b"PrivateBorderColor".to_vec(), PdfString::text("retained"));
    let mut bs = Dict::new();
    let width = doc.add(Object::Int(2));
    bs.set(b"W".to_vec(), Object::Ref(width));
    let style = doc.add(Object::name("D"));
    bs.set(b"S".to_vec(), Object::Ref(style));
    if let Some(dash) = &dash {
        let values: Vec<_> = dash.iter().map(|n| Object::Ref(doc.add(Object::Real(*n)))).collect();
        let array = doc.add(Object::Array(values));
        bs.set(b"D".to_vec(), Object::Ref(array));
    }
    bs.set(b"PrivateBorder".to_vec(), PdfString::text("retained"));
    let bs = doc.add(bs);
    let mk = doc.add(mk);
    // Imported appearance already has the declared custom dash: its construction does
    // not use the product frame() whose later redraw this regression qualifies.
    let mut pattern = Vec::new();
    let expected = dash.unwrap_or_else(|| vec![3.0]);
    pdfcraft_cos::serialize(&Object::Array(expected.iter().copied().map(Object::Real).collect()), &mut pattern);
    let content = format!("0 G 2 w {} 0 d 1 1 158 38 re S [] 0 d\n", String::from_utf8_lossy(&pattern));
    let mut stream_dict = Dict::new();
    stream_dict.set(b"Type".to_vec(), Object::name("XObject"));
    stream_dict.set(b"Subtype".to_vec(), Object::name("Form"));
    stream_dict.set(b"BBox".to_vec(), Object::Array([0, 0, 160, 40].into_iter().map(Object::Int).collect()));
    let stream = doc.add(Object::Stream(Stream::flate(stream_dict, content.as_bytes())));
    let normal = if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
        let mut states = Dict::new();
        states.set(b"Off".to_vec(), Object::Ref(stream));
        states.set(w.on_state.as_deref().unwrap_or("Yes").as_bytes().to_vec(), Object::Ref(stream));
        Object::Dict(states)
    } else {
        Object::Ref(stream)
    };
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), normal);
    ap.set(b"PrivateAppearance".to_vec(), PdfString::text("retained"));
    doc.update_dict(w.obj, |d| {
        d.set(b"MK".to_vec(), Object::Ref(mk));
        d.set(b"BS".to_vec(), Object::Ref(bs));
        d.set(b"AP".to_vec(), Object::Dict(ap));
    })
    .unwrap();
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"OriginalBS".to_vec(), Object::Ref(bs))).unwrap();
    saved(&doc)
}

fn assert_border(stream: &Stream, expected: &[f64]) {
    let parsed = pdfcraft_content::parse(&stream.decoded().unwrap());
    assert_eq!(parsed.skipped, 0);
    let mut dash = Vec::new();
    let mut width = 0.0;
    for op in &parsed.ops {
        if op.is("d") {
            assert_eq!(op.num(1), Some(0.0));
            dash = op.operands.first().unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
        }
        if op.is("w") {
            width = op.num(0).unwrap();
        }
        if op.is("S") {
            assert_eq!(dash, expected);
            assert_eq!(width, 2.0);
            return;
        }
    }
    panic!("border stroke missing");
}

#[test]
fn redraws_emit_imported_dash_lengths_and_width_for_every_widget_kind_after_reopen() {
    for kind in [
        NewField::Text { multiline: false },
        NewField::Combo { options: vec!["A".into()], editable: false },
        NewField::List { options: vec!["A".into()], multi: false },
        NewField::Button { caption: "Submit".into() },
        NewField::Signature,
        NewField::CheckBox,
        NewField::Radio { group: None, export: "Yes".into() },
    ] {
        let expected = vec![1.23456, 0.00001, 5.0, 0.0];
        let mut doc = original(kind, Some(expected.clone()));
        let original_field = field(&doc);
        for stream in normal_streams(&doc, &original_field.widgets[0]) {
            assert_border(&stream, &expected);
        }
        let before_value = original_field.value.clone();
        let rect = original_field.widgets[0].rect;
        let mut selected = look(&doc, &original_field);
        assert_eq!(selected.style, BorderStyle::Dashed);
        assert_eq!(selected.width, 2.0);
        assert_eq!(selected.border, Some([0.0; 3]));
        assert_eq!(selected.fill, Some([1.0; 3]));
        selected.border = Some([0.2, 0.3, 0.4]);
        set_props(&mut doc, "Border", &FieldProps { look: Some(selected), ..FieldProps::default() }).unwrap();
        let doc = saved(&doc);
        let f = field(&doc);
        assert_eq!(f.value, before_value);
        assert_eq!(f.widgets[0].rect, rect);
        for stream in normal_streams(&doc, &f.widgets[0]) {
            assert_border(&stream, &expected);
        }
        let wd = doc.get(f.widgets[0].obj);
        let ap = doc.resolve(wd.as_dict().unwrap().get(b"AP").unwrap());
        assert_eq!(ap.as_dict().unwrap().get(b"PrivateAppearance").unwrap().as_string().unwrap().to_text(), "retained");
        let root = doc.get(doc.root().unwrap());
        let bs = doc.resolve(root.as_dict().unwrap().get(b"OriginalBS").unwrap());
        assert_eq!(bs.as_dict().unwrap().get(b"PrivateBorder").unwrap().as_string().unwrap().to_text(), "retained");
    }
}

#[test]
fn dashed_defaults_empty_patterns_and_single_lengths_keep_their_saved_stroke_state() {
    for pattern in [None, Some(vec![]), Some(vec![5.0]), Some(vec![0.0, 4.0])] {
        let expected = pattern.clone().unwrap_or_else(|| vec![3.0]);
        let mut doc = original(NewField::Text { multiline: false }, pattern);
        set_value(&mut doc, "Border", &FieldValue::Text("Ada".into())).unwrap();
        let doc = saved(&doc);
        assert_eq!(field(&doc).value, ["Ada"]);
        for stream in normal_streams(&doc, &field(&doc).widgets[0]) {
            assert_border(&stream, &expected);
        }
    }
}

#[test]
fn unsupported_dash_redraws_refuse_atomically_without_empty_appearance_success() {
    for value in [Object::Array(vec![Object::Int(0), Object::Int(0)]), Object::Array(vec![Object::Int(-1), Object::Int(2)]), Object::Int(3)] {
        for kind in [NewField::Text { multiline: false }, NewField::Button { caption: "Submit".into() }, NewField::CheckBox] {
            let mut doc = original(kind, Some(vec![5.0, 2.0]));
            let w = field(&doc).widgets.remove(0);
            let bs = doc.get(w.obj).as_dict().unwrap().reference(b"BS").unwrap();
            doc.update_dict(bs, |d| d.set(b"D".to_vec(), value.clone())).unwrap();
            let mut doc = saved(&doc);
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            assert!(redraw_field(&mut doc, "Border").is_err());
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            let f = field(&doc);
            let selected = look(&doc, &f);
            assert!(set_props(&mut doc, "Border", &FieldProps { look: Some(selected), ..FieldProps::default() }).is_err());
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            if f.kind == FieldKind::Text {
                assert!(set_value(&mut doc, "Border", &FieldValue::Text("Ada".into())).is_err());
                assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            }
            if f.kind == FieldKind::CheckBox {
                let before_checked = write_full(&doc, &SaveOptions::default()).unwrap();
                assert!(appearance::check_box_states(&mut doc, &f.widgets[0], f.kind, "Yes").is_err());
                assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before_checked);
                doc.update_dict(f.widgets[0].obj, |d| {
                    d.remove(b"AP");
                })
                .unwrap();
                let mut doc = saved(&doc);
                let before_missing = write_full(&doc, &SaveOptions::default()).unwrap();
                assert!(set_value(&mut doc, "Border", &FieldValue::Check(true)).is_err());
                assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before_missing);
            }
        }
    }
}
