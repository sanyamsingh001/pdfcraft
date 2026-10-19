use super::*;
use pdfcraft_cos::{SaveOptions, write_full};
use std::sync::Arc;

// Contributor-original normal-operation fixture: one blank page, no external assets.
fn blank_page() -> Document {
    let mut doc = Document::new_empty();
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
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

fn widget(doc: &Document, name: &str) -> Widget {
    fields(doc).into_iter().find(|f| f.name == name).unwrap().widgets.remove(0)
}

fn ap(doc: &Document, w: &Widget) -> Dict {
    let d = doc.get(w.obj);
    doc.resolve(d.as_dict().unwrap().get(b"AP").unwrap()).as_dict().unwrap().clone()
}

fn decoded(doc: &Document, object: &Object) -> Vec<u8> {
    let object = doc.resolve(object);
    let Object::Stream(s) = &*object else { panic!("appearance stream") };
    s.decoded().unwrap()
}

#[test]
fn inherited_raw_da_fonts_and_comments_select_the_original_font_after_reopen() {
    for inherited_from_parent in [false, true] {
        let mut doc = blank_page();
        add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
        let w = widget(&doc, "Name");
        doc.update_dict(w.obj, |d| {
            d.remove(b"DA");
        })
        .unwrap();
        let af = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
        let mut fonts = Dict::new();
        fonts.set(b"Field \xffFont".to_vec(), pdfcraft_fonts::generated::standard_font("Courier"));
        let fonts = doc.add(fonts);
        let mut dr = Dict::new();
        dr.set(b"Font".to_vec(), Object::Ref(fonts));
        let dr = doc.add(dr);
        doc.update_dict(af, |d| {
            d.set(b"DA".to_vec(), PdfString::literal(b"/Field#20#FFFont 13 Tf 0.1 0.2 0.3 rg % /Wrong 200 Tf 1 0 0 rg\n".to_vec()));
            d.set(b"DR".to_vec(), Object::Ref(dr));
        })
        .unwrap();
        let name = if inherited_from_parent {
            let da = doc.get(af).as_dict().unwrap().get(b"DA").unwrap().clone();
            let mut parent = Dict::new();
            parent.set(b"T".to_vec(), PdfString::text("Group"));
            parent.set(b"DA".to_vec(), da);
            parent.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(w.obj)]));
            let parent = doc.add(parent);
            doc.update_dict(w.obj, |d| d.set(b"Parent".to_vec(), Object::Ref(parent))).unwrap();
            doc.update_dict(af, |d| {
                d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
                d.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(parent)]));
            })
            .unwrap();
            "Group.Name"
        } else {
            "Name"
        };
        let mut doc = saved(&doc);
        set_value(&mut doc, name, &FieldValue::Text("Grace".into())).unwrap();
        let doc = saved(&doc);
        let w = widget(&doc, name);
        let ap = ap(&doc, &w);
        let object = doc.resolve(ap.get(b"N").unwrap());
        let Object::Stream(stream) = &*object else { panic!() };
        let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        let content = pdfcraft_content::parse(&stream.decoded().unwrap());
        let tf = content.ops.iter().find(|o| o.is("Tf")).unwrap();
        let font = doc.resolve(fonts.as_dict().unwrap().get(tf.name(0).unwrap()).unwrap());
        assert_eq!(font.as_dict().unwrap().name(b"BaseFont"), Some(b"Courier".as_slice()));
        assert_eq!(tf.num(1), Some(13.0));
        assert!(content.ops.iter().any(|o| o.is("rg") && o.nums::<3>() == Some([0.1, 0.2, 0.3])));
        let dr = doc.resolve(acroform(&doc).unwrap().get(b"DR").unwrap());
        let fonts = doc.resolve(dr.as_dict().unwrap().get(b"Font").unwrap());
        assert!(fonts.as_dict().unwrap().contains(b"Field \xffFont"));
        assert_eq!(fields(&doc).into_iter().find(|f| f.name == name).unwrap().value, ["Grace"]);
    }
}

#[test]
fn value_and_author_redraw_preserve_direct_and_shared_indirect_appearance_entries() {
    for indirect in [false, true] {
        for kind in [
            NewField::Text { multiline: false },
            NewField::Combo { options: vec!["Ada".into(), "Grace".into()], editable: false },
            NewField::List { options: vec!["Ada".into(), "Grace".into()], multi: false },
        ] {
            let mut doc = blank_page();
            add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &kind, Some("Name")).unwrap();
            let w = widget(&doc, "Name");
            let mut appearances = ap(&doc, &w);
            let mut dict = Dict::new();
            dict.set(b"Type".to_vec(), Object::name("XObject"));
            dict.set(b"Subtype".to_vec(), Object::name("Form"));
            dict.set(b"BBox".to_vec(), Object::Array([0, 0, 280, 60].into_iter().map(Object::Int).collect()));
            let rollover = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict.clone(), b"% original rollover\n")));
            let down = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict, b"% original down\n")));
            appearances.set(b"R".to_vec(), Object::Ref(rollover));
            appearances.set(b"D".to_vec(), Object::Ref(down));
            appearances.set(b"PrivateAppearance".to_vec(), PdfString::text("retained"));
            let original = doc.add(appearances.clone());
            let object = if indirect { Object::Ref(original) } else { Object::Dict(appearances) };
            doc.update_dict(w.obj, |d| d.set(b"AP".to_vec(), object)).unwrap();
            let root = doc.root().unwrap();
            doc.update_dict(root, |d| d.set(b"OriginalAppearance".to_vec(), Object::Ref(original))).unwrap();
            let mut doc = saved(&doc);
            set_value(&mut doc, "Name", &FieldValue::Text("Grace".into())).unwrap();
            redraw_field(&mut doc, "Name").unwrap();
            let doc = saved(&doc);
            let appearances = ap(&doc, &widget(&doc, "Name"));
            assert_eq!(decoded(&doc, appearances.get(b"R").unwrap()), b"% original rollover\n");
            assert_eq!(decoded(&doc, appearances.get(b"D").unwrap()), b"% original down\n");
            assert_eq!(appearances.get(b"PrivateAppearance").unwrap().as_string().unwrap().to_text(), "retained");
            let catalog = doc.get(doc.root().unwrap());
            let original = doc.resolve(catalog.as_dict().unwrap().get(b"OriginalAppearance").unwrap());
            let original = original.as_dict().unwrap();
            assert_ne!(decoded(&doc, original.get(b"N").unwrap()), decoded(&doc, appearances.get(b"N").unwrap()));
            assert_eq!(decoded(&doc, original.get(b"R").unwrap()), b"% original rollover\n");
        }
    }
}

#[test]
fn refused_form_appearance_keeps_values_resources_and_shared_appearances() {
    let mut doc = blank_page();
    add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    set_value(&mut doc, "Name", &FieldValue::Text("Ada".into())).unwrap();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(set_value(&mut doc, "Name", &FieldValue::Text("🙂".into())).is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    let w = widget(&doc, "Name");
    doc.update_dict(w.obj, |d| d.set(b"DA".to_vec(), PdfString::literal(vec![b' '; 65_537]))).unwrap();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(set_value(&mut doc, "Name", &FieldValue::Text("Grace".into())).is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn nondefault_or_nonstring_field_da_refuses_value_and_author_changes_atomically() {
    for state in ["1 Tc", "2 Tw", "80 Tz", "1 Tr", "2 Ts", "14 TL"] {
        for inherited in [false, true] {
            let mut doc = blank_page();
            add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
            set_value(&mut doc, "Name", &FieldValue::Text("Ada".into())).unwrap();
            let w = widget(&doc, "Name");
            let da = PdfString::literal(format!("/Helv 12 Tf 0 g {state}").into_bytes());
            if inherited {
                doc.update_dict(w.obj, |d| {
                    d.remove(b"DA");
                })
                .unwrap();
                let af = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
                doc.update_dict(af, |d| d.set(b"DA".to_vec(), da)).unwrap();
            } else {
                doc.update_dict(w.obj, |d| d.set(b"DA".to_vec(), da)).unwrap();
            }
            let mut doc = saved(&doc);
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            assert!(set_value(&mut doc, "Name", &FieldValue::Text("Grace".into())).is_err(), "{state}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert!(set_props(&mut doc, "Name", &FieldProps { font_size: Some(14.0), ..FieldProps::default() }).is_err(), "{state}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert!(redraw_field(&mut doc, "Name").is_err(), "{state}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
    let mut doc = blank_page();
    add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    let w = widget(&doc, "Name");
    doc.update_dict(w.obj, |d| d.set(b"DA".to_vec(), Object::Int(7))).unwrap();
    let mut doc = saved(&doc);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(set_value(&mut doc, "Name", &FieldValue::Text("Grace".into())).is_err());
    assert!(set_props(&mut doc, "Name", &FieldProps { font_size: Some(14.0), ..FieldProps::default() }).is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn explicit_default_field_text_state_keeps_persisted_font_size_and_content() {
    let mut doc = blank_page();
    add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    let w = widget(&doc, "Name");
    doc.update_dict(w.obj, |d| {
        d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 13 Tf 0 g 0 Tc 0 Tw 100 Tz 0 Tr 0 Ts 0 TL % 3 Tc\n q /Helv 99 Tf 1 g Q".to_vec()))
    })
    .unwrap();
    let mut doc = saved(&doc);
    set_value(&mut doc, "Name", &FieldValue::Text("III".into())).unwrap();
    let doc = saved(&doc);
    let appearance = ap(&doc, &widget(&doc, "Name"));
    let object = doc.resolve(appearance.get(b"N").unwrap());
    let Object::Stream(stream) = &*object else { panic!() };
    let parsed = pdfcraft_content::parse(&stream.decoded().unwrap());
    let tf = parsed.ops.iter().find(|o| o.is("Tf")).unwrap();
    assert_eq!(tf.num(1), Some(13.0));
    let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(tf.name(0).unwrap()).unwrap());
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font.as_dict().unwrap());
    let shown: String =
        parsed.ops.iter().filter(|o| o.is("Tj")).map(|o| metrics.decode(&o.operands.first().unwrap().as_string().unwrap().bytes)).collect();
    assert_eq!(shown, "III");
    assert!((metrics.width(u32::from(b'I')) * 13.0 * 3.0 - 10.842).abs() < 1e-9);
    assert!(parsed.ops.iter().any(|o| o.is("g") && o.num(0) == Some(0.0)));
}
