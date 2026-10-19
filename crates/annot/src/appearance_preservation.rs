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

fn meta() -> Meta {
    Meta { date: Some("D:20261008120000Z".into()), id: "original-preservation".into() }
}

// These fixtures create indirect annotations. Inspection must not use annot_ref,
// which can promote direct entries and thereby alter the document under test.
fn fixture_annotation_ref(doc: &Document) -> ObjRef {
    annots(doc, page_ref(doc, 0).unwrap()).first().unwrap().as_ref().unwrap()
}

fn normal(doc: &Document) -> (Dict, pdfcraft_cos::Stream) {
    let r = fixture_annotation_ref(doc);
    let d = annot_dict(doc, r);
    let ap = doc.resolve(d.get(b"AP").unwrap());
    let n = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = &*n else { panic!("normal appearance") };
    (d, stream.clone())
}

#[test]
fn escaped_raw_freetext_fonts_comments_and_occupied_states_survive_restyle() {
    let mut doc = blank_page();
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 13.0 },
            style: Style { color: [0.1, 0.2, 0.3], opacity: 1.0, width: 0.0, fill: None },
            contents: "Ada".into(),
            author: "Original".into(),
        },
        &meta(),
    )
    .unwrap();
    let r = fixture_annotation_ref(&doc);
    let (_, mut ap) = normal(&doc);
    let mut fonts = Dict::new();
    fonts.set(b"Field \xffFont".to_vec(), pdfcraft_fonts::generated::standard_font("Helvetica"));
    // Preoccupy the safe alias too; the generated font must reserve PCDA_.
    fonts.set(b"PCDA".to_vec(), pdfcraft_fonts::generated::standard_font("Courier"));
    let fonts = doc.add(fonts);
    let mut old_state = Dict::new();
    old_state.set(b"CA".to_vec(), Object::Real(0.75));
    old_state.set(b"PrivateState".to_vec(), PdfString::text("retained"));
    let old_state = doc.add(old_state);
    let mut states = Dict::new();
    states.set(b"GS0".to_vec(), Object::Ref(old_state));
    let states = doc.add(states);
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Ref(fonts));
    resources.set(b"ExtGState".to_vec(), Object::Ref(states));
    ap.dict.set(b"Resources".to_vec(), Object::Ref(doc.add(resources)));
    let ap = doc.add(Object::Stream(ap));
    let mut apd = Dict::new();
    apd.set(b"N".to_vec(), Object::Ref(ap));
    doc.update_dict(r, |d| {
        d.set(b"AP".to_vec(), Object::Dict(apd));
        d.set(b"DA".to_vec(), PdfString::literal(b"/Field#20#FFFont 13 Tf 0.1 0.2 0.3 rg % /Wrong 200 Tf 1 0 0 rg\n".to_vec()));
    })
    .unwrap();
    // Preserve shared imported dictionaries independently of the regenerated appearance.
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(b"OriginalFonts".to_vec(), Object::Ref(fonts));
        d.set(b"OriginalStates".to_vec(), Object::Ref(states));
    })
    .unwrap();
    let mut doc = saved(&doc);
    assert_eq!(appearance::parse_da(&normal(&doc).0), ([0.1, 0.2, 0.3], 13.0));
    set_contents(&mut doc, 0, 0, "Grace", &meta()).unwrap();
    set_style(&mut doc, 0, 0, Some([0.2, 0.3, 0.4]), Some(0.5), None, &meta()).unwrap();
    let doc = saved(&doc);
    let (d, ap) = normal(&doc);
    assert_eq!(appearance::da_font(&d).unwrap(), b"Field \xffFont");
    assert_eq!(appearance::parse_da(&d), ([0.2, 0.3, 0.4], 13.0));
    let da = d.get(b"DA").unwrap().as_string().unwrap();
    assert!(da.bytes.windows(b"/Field#20#FFFont 13 Tf".len()).any(|s| s == b"/Field#20#FFFont 13 Tf"));
    let resources = doc.resolve(ap.dict.get(b"Resources").unwrap());
    let resources = resources.as_dict().unwrap();
    let fonts = doc.resolve(resources.get(b"Font").unwrap());
    let fonts = fonts.as_dict().unwrap();
    assert_eq!(doc.resolve(fonts.get(b"Field \xffFont").unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(b"Helvetica".as_slice()));
    assert_eq!(doc.resolve(fonts.get(b"PCDA").unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(b"Courier".as_slice()));
    let content = pdfcraft_content::parse(&ap.decoded().unwrap());
    let tf = content.ops.iter().find(|o| o.is("Tf")).unwrap();
    assert_eq!(tf.name(0), Some(b"PCDA_".as_slice()));
    assert_eq!(tf.num(1), Some(13.0));
    let gs = content.ops.iter().find(|o| o.is("gs")).unwrap().name(0).unwrap();
    assert_eq!(gs, b"GS0_");
    let states = doc.resolve(resources.get(b"ExtGState").unwrap());
    let states = states.as_dict().unwrap();
    let original = doc.resolve(states.get(b"GS0").unwrap());
    let original = original.as_dict().unwrap();
    assert_eq!(original.get(b"PrivateState").unwrap().as_string().unwrap().to_text(), "retained");
    assert_eq!(original.get(b"CA").unwrap().as_f64(), Some(0.75));
    assert_eq!(doc.resolve(states.get(gs).unwrap()).as_dict().unwrap().get(b"ca").unwrap().as_f64(), Some(0.5));
    let catalog = doc.get(doc.root().unwrap());
    let catalog = catalog.as_dict().unwrap();
    let old_states = doc.resolve(catalog.get(b"OriginalStates").unwrap());
    assert_eq!(old_states.as_dict().unwrap().len(), 1);
    let old_fonts = doc.resolve(catalog.get(b"OriginalFonts").unwrap());
    assert_eq!(old_fonts.as_dict().unwrap().len(), 2);
}

#[test]
fn unreadable_freetext_da_refusal_preserves_the_document() {
    let mut doc = blank_page();
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 12.0 },
            style: Style::default(),
            contents: "Ada".into(),
            author: "Original".into(),
        },
        &meta(),
    )
    .unwrap();
    let r = fixture_annotation_ref(&doc);
    doc.update_dict(r, |d| d.set(b"DA".to_vec(), PdfString::literal(b"(unfinished".to_vec()))).unwrap();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(set_contents(&mut doc, 0, 0, "Grace", &meta()).is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn rich_text_string_notation_does_not_change_supported_editability() {
    for initial in ["A ŠœŸ €", "ASCII"] {
        for force_hex in [false, true] {
            let mut doc = blank_page();
            add_annotation(
                &mut doc,
                &NewAnnotation {
                    page: 0,
                    shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 12.0 },
                    style: Style::default(),
                    contents: initial.into(),
                    author: "Original".into(),
                },
                &meta(),
            )
            .unwrap();
            let r = fixture_annotation_ref(&doc);
            let before = annot_dict(&doc, r);
            let original_rc = before.get(b"RC").unwrap().as_string().unwrap().bytes.clone();
            let original_ds = before.get(b"DS").unwrap().as_string().unwrap().bytes.clone();
            if force_hex {
                doc.update_dict(r, |d| {
                    for key in [b"RC".as_slice(), b"DS".as_slice()] {
                        let mut s = d.get(key).unwrap().as_string().unwrap().clone();
                        s.hex = true;
                        d.set(key.to_vec(), s);
                    }
                })
                .unwrap();
            }
            let mut doc = saved(&doc);
            let (d, _) = normal(&doc);
            assert_eq!(d.get(b"RC").unwrap().as_string().unwrap().bytes, original_rc);
            assert_eq!(d.get(b"DS").unwrap().as_string().unwrap().bytes, original_ds);
            if force_hex || initial != "ASCII" {
                assert!(d.get(b"RC").unwrap().as_string().unwrap().hex, "notation changed or was explicitly hex");
            }
            assert!(props(&doc, 0, 0).unwrap().restylable);
            set_contents(&mut doc, 0, 0, "Žžš Œ", &meta()).unwrap();
            set_style(&mut doc, 0, 0, Some([0.3, 0.2, 0.1]), Some(0.5), None, &meta()).unwrap();
            let doc = saved(&doc);
            assert!(props(&doc, 0, 0).unwrap().restylable);
            let (d, _) = normal(&doc);
            assert_eq!(d.get(b"Contents").unwrap().as_string().unwrap().to_text(), "Žžš Œ");
            assert!(d.get(b"RC").unwrap().as_string().unwrap().to_text().contains("Žžš Œ"));
        }
    }
}

#[test]
fn distinct_or_nonstring_rich_styles_remain_atomic_refusals_after_reopen() {
    for rich in [Object::String(PdfString::text("<body><b>Ada</b></body>")), Object::Int(7)] {
        let mut doc = blank_page();
        add_annotation(
            &mut doc,
            &NewAnnotation {
                page: 0,
                shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 12.0 },
                style: Style::default(),
                contents: "Ada".into(),
                author: "Original".into(),
            },
            &meta(),
        )
        .unwrap();
        let r = fixture_annotation_ref(&doc);
        doc.update_dict(r, |d| {
            d.set(b"RC".to_vec(), rich);
            let mut ds = d.get(b"DS").unwrap().as_string().unwrap().clone();
            ds.hex = true;
            d.set(b"DS".to_vec(), ds);
        })
        .unwrap();
        let mut doc = saved(&doc);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(!props(&doc, 0, 0).unwrap().restylable);
        assert!(set_contents(&mut doc, 0, 0, "Grace", &meta()).is_err());
        assert!(set_style(&mut doc, 0, 0, Some([1.0, 0.0, 0.0]), Some(0.5), None, &meta()).is_err());
        assert!(set_rect(&mut doc, 0, 0, [20.0, 20.0, 320.0, 80.0], &meta()).is_err());
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn nondefault_freetext_da_keeps_imported_appearance_on_every_redraw_refusal() {
    for state in ["1 Tc", "2 Tw", "80 Tz", "1 Tr", "2 Ts", "14 TL"] {
        let mut doc = blank_page();
        add_annotation(
            &mut doc,
            &NewAnnotation {
                page: 0,
                shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 12.0 },
                style: Style::default(),
                contents: "Ada".into(),
                author: "Original".into(),
            },
            &meta(),
        )
        .unwrap();
        let r = fixture_annotation_ref(&doc);
        let (_, mut appearance) = normal(&doc);
        let bytes = appearance.decoded().unwrap();
        let content = pdfcraft_content::parse(&bytes);
        let tf = content.ops.iter().find(|o| o.is("Tf")).unwrap();
        let mut styled = bytes.get(..tf.span.end).unwrap().to_vec();
        styled.extend(format!(" {state} ").bytes());
        styled.extend_from_slice(bytes.get(tf.span.end..).unwrap());
        appearance = pdfcraft_cos::Stream::flate(appearance.dict.clone(), &styled);
        let appearance = doc.add(Object::Stream(appearance));
        doc.update_dict(r, |d| {
            d.remove(b"RC");
            d.remove(b"DS");
            d.set(b"DA".to_vec(), PdfString::literal(format!("/Helv 12 Tf 0 g {state}").into_bytes()));
            let mut ap = Dict::new();
            ap.set(b"N".to_vec(), Object::Ref(appearance));
            d.set(b"AP".to_vec(), Object::Dict(ap));
        })
        .unwrap();
        let mut doc = saved(&doc);
        assert_eq!(normal(&doc).1.decoded().unwrap(), styled);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(set_contents(&mut doc, 0, 0, "Grace", &meta()).is_err(), "{state}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        assert!(set_style(&mut doc, 0, 0, Some([0.2, 0.3, 0.4]), None, None, &meta()).is_err(), "{state}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        assert!(set_rect(&mut doc, 0, 0, [20.0, 20.0, 320.0, 100.0], &meta()).is_err(), "{state}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        doc.update_dict(r, |d| d.set(b"DA".to_vec(), Object::Int(7))).unwrap();
        let mut doc = saved(&doc);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(set_contents(&mut doc, 0, 0, "Grace", &meta()).is_err());
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn explicit_default_freetext_text_state_keeps_persisted_font_size_and_content() {
    let mut doc = blank_page();
    add_annotation(
        &mut doc,
        &NewAnnotation {
            page: 0,
            shape: Shape::TextBox { rect: [20.0, 20.0, 300.0, 80.0], font_size: 13.0 },
            style: Style::default(),
            contents: "Ada".into(),
            author: "Original".into(),
        },
        &meta(),
    )
    .unwrap();
    let r = fixture_annotation_ref(&doc);
    doc.update_dict(r, |d| {
        d.remove(b"RC");
        d.remove(b"DS");
        d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 13 Tf 0.2 g 0 Tc 0 Tw 100 Tz 0 Tr 0 Ts 0 TL % 3 Tc\n q /Helv 99 Tf 1 g Q".to_vec()));
    })
    .unwrap();
    let mut doc = saved(&doc);
    set_contents(&mut doc, 0, 0, "III", &meta()).unwrap();
    set_style(&mut doc, 0, 0, Some([0.2, 0.3, 0.4]), None, None, &meta()).unwrap();
    let doc = saved(&doc);
    let (_, stream) = normal(&doc);
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
    assert!(parsed.ops.iter().any(|o| o.is("rg") && o.nums::<3>() == Some([0.2, 0.3, 0.4])));
}
