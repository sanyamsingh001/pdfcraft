use super::*;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use std::sync::Arc;

// Contributor-original two-page documents with only standard technical fonts and vector icons.
fn blank() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut kids = Vec::new();
    for _ in 0..2 {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 600, 800].into_iter().map(Object::Int).collect()));
        kids.push(Object::Ref(doc.add(page)));
    }
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc
}

fn saved(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn field(doc: &Document, name: &str) -> Field {
    fields(doc).into_iter().find(|f| f.name == name).unwrap()
}

fn appearance(doc: &Document, w: &Widget) -> Stream {
    let object = doc.get(w.obj);
    let ap = doc.resolve(object.as_dict().unwrap().get(b"AP").unwrap());
    let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let Object::Stream(stream) = &*normal else { panic!("normal appearance") };
    stream.clone()
}

fn resolved_dict(doc: &Document, obj: ObjRef, key: &[u8]) -> Dict {
    let object = doc.get(obj);
    doc.resolve(object.as_dict().unwrap().get(key).unwrap()).as_dict().unwrap().clone()
}

#[test]
fn size_only_edits_preserve_distinct_raw_widget_fonts_colors_and_geometry_after_reopen() {
    let mut doc = blank();
    add_field(&mut doc, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    duplicate_field(&mut doc, "Name", &[1]).unwrap();
    let original = field(&doc, "Name");
    let af = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
    let mut dr = resolved_dict(&doc, af, b"DR");
    let mut fonts = doc.resolve(dr.get(b"Font").unwrap()).as_dict().unwrap().clone();
    fonts.set(b"F1".to_vec(), pdfcraft_fonts::generated::standard_font("Courier"));
    fonts.set(b"Field \xffFont".to_vec(), pdfcraft_fonts::generated::standard_font("Helvetica"));
    let shared_fonts = doc.add(fonts);
    dr.set(b"Font".to_vec(), Object::Ref(shared_fonts));
    doc.update_dict(af, |d| d.set(b"DR".to_vec(), Object::Dict(dr))).unwrap();
    doc.update_dict(original.obj, |d| {
        d.set(b"DA".to_vec(), PdfString::literal(b"/F1 12 Tf 0.25 g".to_vec()));
        d.set(b"Q".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc.update_dict(original.widgets[0].obj, |d| {
        d.remove(b"DA");
    })
    .unwrap();
    doc.update_dict(original.widgets[1].obj, |d| d.set(b"DA".to_vec(), PdfString::literal(b"/Field#20#FFFont 11 Tf 0.1 0.2 0.3 rg".to_vec())))
        .unwrap();
    set_value(&mut doc, "Name", &FieldValue::Text("III".into())).unwrap();
    let mut doc = saved(&doc);
    let before = field(&doc, "Name");
    let original_rects: Vec<_> = before.widgets.iter().map(|w| w.rect).collect();
    set_props(&mut doc, "Name", &FieldProps { font_size: Some(14.0), ..FieldProps::default() }).unwrap();
    let doc = saved(&doc);
    let f = field(&doc, "Name");
    assert_eq!(f.value, ["III"]);
    assert_eq!(f.widgets.iter().map(|w| w.rect).collect::<Vec<_>>(), original_rects);
    let field_da = doc.get(f.obj).as_dict().unwrap().get(b"DA").unwrap().as_string().unwrap().bytes.clone();
    assert_eq!(pdfcraft_content::default_appearance::parse_default_appearance(&field_da).unwrap().font.as_deref(), Some(b"F1".as_slice()));
    for (w, raw, face, color, advance) in [
        (&f.widgets[0], b"F1".as_slice(), b"Courier".as_slice(), vec![0.25], 0.6),
        (&f.widgets[1], b"Field \xffFont".as_slice(), b"Helvetica".as_slice(), vec![0.1, 0.2, 0.3], 0.278),
    ] {
        let object = doc.get(w.obj);
        let da = object.as_dict().unwrap().get(b"DA").unwrap().as_string().unwrap();
        let parsed = pdfcraft_content::default_appearance::parse_default_appearance(&da.bytes).unwrap();
        assert_eq!(parsed.font.as_deref(), Some(raw));
        assert_eq!(parsed.size, Some(14.0));
        assert_eq!(parsed.color, Some(color.clone()));
        let stream = appearance(&doc, w);
        let parsed = pdfcraft_content::parse(&stream.decoded().unwrap());
        let tf = parsed.ops.iter().find(|o| o.is("Tf")).unwrap();
        assert_eq!(tf.num(1), Some(14.0));
        let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        let font = doc.resolve(fonts.as_dict().unwrap().get(tf.name(0).unwrap()).unwrap());
        assert_eq!(font.as_dict().unwrap().name(b"BaseFont"), Some(face));
        let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font.as_dict().unwrap());
        assert!((metrics.width(u32::from(b'I')) - advance).abs() < 1e-9);
        let shown: String =
            parsed.ops.iter().filter(|o| o.is("Tj")).map(|o| metrics.decode(&o.operands.first().unwrap().as_string().unwrap().bytes)).collect();
        assert_eq!(shown, "III");
        let tm = parsed.ops.iter().find(|o| o.is("Tm")).unwrap().nums::<6>().unwrap();
        assert!((tm[4] - (280.0 - 3.0 - advance * 14.0 * 3.0)).abs() < 0.0006);
        assert!(parsed.ops.iter().any(|o| {
            if color.len() == 1 {
                o.is("g") && o.num(0) == Some(color[0])
            } else {
                o.is("rg") && o.nums::<3>() == Some([color[0], color[1], color[2]])
            }
        }));
    }
    // Explicit selection still chooses the requested face, rather than retaining aliases.
    let mut doc = doc;
    let mut selected = look(&doc, &f);
    selected.font = FieldFont::Times;
    set_props(&mut doc, "Name", &FieldProps { look: Some(selected), ..FieldProps::default() }).unwrap();
    let doc = saved(&doc);
    for w in &field(&doc, "Name").widgets {
        let stream = appearance(&doc, w);
        let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        assert_eq!(doc.resolve(fonts.as_dict().unwrap().get(b"TiRo").unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(b"Times-Roman".as_slice()));
    }
}

fn imported_buttons(indirect: bool) -> Document {
    let mut doc = blank();
    for name in ["Action", "Other"] {
        add_field(&mut doc, 0, [20.0, 100.0, 180.0, 140.0], &NewField::Button { caption: "Submit".into() }, Some(name)).unwrap();
    }
    let mut icon = Dict::new();
    icon.set(b"Type".to_vec(), Object::name("XObject"));
    icon.set(b"Subtype".to_vec(), Object::name("Form"));
    icon.set(b"BBox".to_vec(), Object::Array([0, 0, 10, 10].into_iter().map(Object::Int).collect()));
    let icon = doc.add(Object::Stream(Stream::flate(icon, b"0.4 g 0 0 10 10 re f\n")));
    let mut mk = Dict::new();
    mk.set(b"CA".to_vec(), PdfString::text("Submit"));
    mk.set(b"I".to_vec(), Object::Ref(icon));
    mk.set(b"TP".to_vec(), Object::Int(0));
    mk.set(b"BC".to_vec(), Object::Array(vec![Object::Real(0.0); 3]));
    mk.set(b"BG".to_vec(), Object::Array(vec![Object::Real(1.0); 3]));
    mk.set(b"PrivateCaption".to_vec(), PdfString::text("retained"));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Int(1));
    bs.set(b"S".to_vec(), Object::name("D"));
    bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(5), Object::Int(2)]));
    bs.set(b"PrivateBorder".to_vec(), PdfString::text("retained"));
    let old_mk = doc.add(mk.clone());
    let old_bs = doc.add(bs.clone());
    for name in ["Action", "Other"] {
        let w = field(&doc, name).widgets.remove(0);
        doc.update_dict(w.obj, |d| {
            d.set(b"MK".to_vec(), if indirect { Object::Ref(old_mk) } else { Object::Dict(mk.clone()) });
            d.set(b"BS".to_vec(), if indirect { Object::Ref(old_bs) } else { Object::Dict(bs.clone()) });
        })
        .unwrap();
        redraw_field(&mut doc, name).unwrap();
    }
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(b"OriginalMK".to_vec(), Object::Ref(old_mk));
        d.set(b"OriginalBS".to_vec(), Object::Ref(old_bs));
    })
    .unwrap();
    saved(&doc)
}

#[test]
fn look_edits_keep_shared_button_captions_icons_dash_patterns_and_private_entries() {
    for indirect in [false, true] {
        let mut doc = imported_buttons(indirect);
        let other = field(&doc, "Other").widgets.remove(0);
        let original_other = doc.get(other.obj).as_dict().unwrap().clone();
        let old_mk = resolved_dict(&doc, other.obj, b"MK");
        let mut old_mk_without_icon = old_mk.clone();
        old_mk_without_icon.remove(b"I");
        let old_bs = resolved_dict(&doc, other.obj, b"BS");
        let original_other_ap = appearance(&doc, &other).decoded().unwrap();
        let mut selected = look(&doc, &field(&doc, "Action"));
        selected.border = Some([0.2, 0.3, 0.4]);
        selected.width = 2.0;
        set_props(&mut doc, "Action", &FieldProps { look: Some(selected), ..FieldProps::default() }).unwrap();
        let doc = saved(&doc);
        let action = field(&doc, "Action").widgets.remove(0);
        let other = field(&doc, "Other").widgets.remove(0);
        let mk = resolved_dict(&doc, action.obj, b"MK");
        let bs = resolved_dict(&doc, action.obj, b"BS");
        for key in [b"CA".as_slice(), b"TP", b"PrivateCaption"] {
            assert_eq!(mk.get(key), old_mk.get(key));
        }
        assert!(mk.get(b"I").unwrap().as_ref().is_some());
        let icon = doc.resolve(mk.get(b"I").unwrap());
        let Object::Stream(icon) = &*icon else { panic!() };
        assert_eq!(icon.decoded().unwrap(), b"0.4 g 0 0 10 10 re f\n");
        assert_eq!(bs.get(b"PrivateBorder"), old_bs.get(b"PrivateBorder"));
        assert_eq!(bs.get(b"D"), old_bs.get(b"D"));
        assert_eq!(bs.get(b"W").unwrap().as_f64(), Some(2.0));
        let mut other_mk = resolved_dict(&doc, other.obj, b"MK");
        assert_eq!(mk.get(b"I"), other_mk.get(b"I"));
        other_mk.remove(b"I");
        assert_eq!(other_mk, old_mk_without_icon);
        assert_eq!(resolved_dict(&doc, other.obj, b"BS"), old_bs);
        assert_eq!(appearance(&doc, &other).decoded().unwrap(), original_other_ap);
        // Catalog-shared dictionaries remain the original dictionaries, not edited aliases.
        let root = doc.get(doc.root().unwrap());
        let mut catalog_mk = doc.resolve(root.as_dict().unwrap().get(b"OriginalMK").unwrap()).as_dict().unwrap().clone();
        assert_eq!(catalog_mk.get(b"I"), mk.get(b"I"));
        catalog_mk.remove(b"I");
        assert_eq!(catalog_mk, old_mk_without_icon);
        assert_eq!(doc.resolve(root.as_dict().unwrap().get(b"OriginalBS").unwrap()).as_dict().unwrap(), &old_bs);
        assert_eq!(doc.get(other.obj).as_dict().unwrap().get(b"T"), original_other.get(b"T"));
        let parsed = pdfcraft_content::parse(&appearance(&doc, &action).decoded().unwrap());
        assert!(parsed.ops.iter().any(|o| o.is("Do") && o.name(0) == Some(b"Icon".as_slice())));
        assert!(parsed.ops.iter().any(|o| o.is("Tj") && o.operands.first().and_then(Object::as_string).is_some_and(|s| s.bytes == b"Submit")));
    }
}

#[test]
fn failed_author_edits_preserve_values_and_shared_dictionaries_byte_for_byte() {
    let mut doc = imported_buttons(true);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let mut selected = look(&doc, &field(&doc, "Action"));
    selected.width = 2.0;
    assert!(
        set_props(&mut doc, "Action", &FieldProps { look: Some(selected), rect: Some((0, [0.0, 0.0, 2.0, 2.0])), ..FieldProps::default() }).is_err()
    );
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    let mut text = blank();
    add_field(&mut text, 0, [20.0, 20.0, 300.0, 80.0], &NewField::Text { multiline: false }, Some("Name")).unwrap();
    set_value(&mut text, "Name", &FieldValue::Text("Ada".into())).unwrap();
    let mut text = saved(&text);
    let before = write_full(&text, &SaveOptions::default()).unwrap();
    assert!(set_props(&mut text, "Name", &FieldProps { font_size: Some(f64::NAN), ..FieldProps::default() }).is_err());
    assert_eq!(write_full(&text, &SaveOptions::default()).unwrap(), before);
    let f = field(&text, "Name");
    text.update_dict(f.obj, |d| d.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 1 Tc 0 g".to_vec()))).unwrap();
    let mut text = saved(&text);
    let before = write_full(&text, &SaveOptions::default()).unwrap();
    assert!(set_props(&mut text, "Name", &FieldProps { font_size: Some(14.0), ..FieldProps::default() }).is_err());
    assert_eq!(write_full(&text, &SaveOptions::default()).unwrap(), before);
}

/// Start from a valid imported button appearance, then represent equivalent MK leaf values
/// indirectly without regenerating it. The second button and catalog share the original MK.
fn imported_button_leaf_values(indirect_caption: bool, indirect_layout: bool, layout: i64) -> Document {
    let mut doc = imported_buttons(true);
    let shared = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"OriginalMK").unwrap();
    doc.update_dict(shared, |d| d.set(b"TP".to_vec(), Object::Int(layout))).unwrap();
    for name in ["Action", "Other"] {
        redraw_field(&mut doc, name).unwrap();
    }
    let caption = if indirect_caption { Object::Ref(doc.add(PdfString::text("Submit"))) } else { Object::String(PdfString::text("Submit")) };
    let layout = if indirect_layout { Object::Ref(doc.add(Object::Int(layout))) } else { Object::Int(layout) };
    doc.update_dict(shared, |d| {
        d.set(b"CA".to_vec(), caption);
        d.set(b"TP".to_vec(), layout);
    })
    .unwrap();
    saved(&doc)
}

fn border_only_button_edit(doc: &mut Document) {
    let mut selected = look(doc, &field(doc, "Action"));
    selected.border = Some([0.2, 0.3, 0.4]);
    set_props(doc, "Action", &FieldProps { look: Some(selected), ..Default::default() }).unwrap();
}

fn assert_shared_button_metadata(doc: &Document, indirect_caption: bool, indirect_layout: bool, layout: i64, other_appearance: &[u8]) {
    let action = field(doc, "Action").widgets.remove(0);
    let other = field(doc, "Other").widgets.remove(0);
    let root = doc.get(doc.root().unwrap());
    let original_ref = root.as_dict().unwrap().get(b"OriginalMK").unwrap();
    let original = doc.resolve(original_ref).as_dict().unwrap().clone();
    let other_object = doc.get(other.obj);
    assert_eq!(other_object.as_dict().unwrap().get(b"MK"), Some(original_ref), "untouched button still shares the original dictionary");
    assert_eq!(resolved_dict(doc, other.obj, b"MK"), original);
    assert_eq!(appearance(doc, &other).decoded().unwrap(), other_appearance);
    let mut changed = resolved_dict(doc, action.obj, b"MK");
    assert_eq!(doc.resolve(changed.get(b"CA").unwrap()).as_string().unwrap().to_text(), "Submit");
    assert_eq!(doc.resolve(changed.get(b"TP").unwrap()).as_int(), Some(layout));
    assert_eq!(changed.get(b"CA").unwrap().as_ref().is_some(), indirect_caption);
    assert_eq!(changed.get(b"TP").unwrap().as_ref().is_some(), indirect_layout);
    assert_eq!(original.get(b"BC").unwrap().as_array().unwrap().iter().map(Object::as_f64).collect::<Vec<_>>(), vec![Some(0.0); 3]);
    assert_eq!(changed.get(b"BC").unwrap().as_array().unwrap().iter().map(Object::as_f64).collect::<Vec<_>>(), [Some(0.2), Some(0.3), Some(0.4)]);
    assert_eq!(original.get(b"PrivateCaption").unwrap().as_string().unwrap().to_text(), "retained");
    // Compare aliases only inside this reopened document; full save may renumber references.
    changed.remove(b"BC");
    let mut unchanged = original;
    unchanged.remove(b"BC");
    assert_eq!(changed, unchanged, "caption/layout/icon aliases and unknown MK entries survive the unrelated edit");
    let ap = appearance(doc, &action);
    let resources = doc.resolve(ap.dict.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    assert_eq!(xobjects.as_dict().unwrap().get(b"Icon"), unchanged.get(b"I"));
    let icon = doc.resolve(unchanged.get(b"I").unwrap());
    let Object::Stream(icon) = &*icon else { panic!("vector icon") };
    assert_eq!(icon.decoded().unwrap(), b"0.4 g 0 0 10 10 re f\n");
}

#[test]
fn unrelated_button_look_edits_preserve_indirect_captions_and_shared_metadata() {
    for indirect_caption in [false, true] {
        let mut doc = imported_button_leaf_values(indirect_caption, false, 0);
        let other = appearance(&doc, &field(&doc, "Other").widgets[0]).decoded().unwrap();
        border_only_button_edit(&mut doc);
        let doc = saved(&doc);
        assert_shared_button_metadata(&doc, indirect_caption, false, 0, &other);
        let normal = appearance(&doc, &field(&doc, "Action").widgets[0]);
        let parsed = pdfcraft_content::parse(&normal.decoded().unwrap());
        let shown: Vec<_> = parsed.ops.iter().filter(|o| o.is("Tj")).map(|o| o.operands[0].as_string().unwrap().to_text()).collect();
        assert_eq!(shown, ["Submit"], "the saved normal appearance still paints the caption");
        assert!(parsed.ops.iter().any(|o| o.is("Do") && o.name(0) == Some(b"Icon".as_slice())));
    }
}

#[test]
fn unrelated_button_look_edits_honor_indirect_icon_only_layout() {
    for indirect_layout in [false, true] {
        // Keep CA direct and nonempty: a lost indirect caption must not mask a TP failure.
        let mut doc = imported_button_leaf_values(false, indirect_layout, 1);
        let other = appearance(&doc, &field(&doc, "Other").widgets[0]).decoded().unwrap();
        border_only_button_edit(&mut doc);
        let doc = saved(&doc);
        assert_shared_button_metadata(&doc, false, indirect_layout, 1, &other);
        let normal = appearance(&doc, &field(&doc, "Action").widgets[0]);
        let parsed = pdfcraft_content::parse(&normal.decoded().unwrap());
        assert!(!parsed.ops.iter().any(|o| o.is("BT") || o.is("Tj") || o.is("TJ")), "icon-only layout must not paint its stored caption");
        assert!(parsed.ops.iter().any(|o| o.is("Do") && o.name(0) == Some(b"Icon".as_slice())), "icon-only layout still draws the vector icon");
    }
}
