//! Contributor-original ordinary AcroForm defaults and resource collisions.
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_full};
use pdfcraft_forms::{FieldValue, fields, set_value};

use super::{combine_selected, extract_pages, import_pages};

fn dict(entries: &[(&[u8], Object)]) -> Dict {
    entries.iter().map(|(key, value)| (key.to_vec(), value.clone())).collect()
}

fn fixture(name: &str, base_font: &str, catalog_defaults: bool) -> Document {
    let mut doc = Document::new_empty();
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let font = doc.add(dict(&[
        (b"Type", Object::name("Font")),
        (b"Subtype", Object::name("Type1")),
        (b"BaseFont", Object::name(base_font)),
        (b"Encoding", Object::name("WinAnsiEncoding")),
        (b"OriginalFontMetadata", Object::String(PdfString::text(base_font))),
    ]));
    let fonts = doc.add(dict(&[(b"F Font", Object::Ref(font))]));
    let dr = doc.add(dict(&[(b"Font", Object::Ref(fonts)), (b"OriginalResourceMetadata", Object::String(PdfString::text("preserved")))]));
    let da = doc.add(Object::String(PdfString::literal(b"/F#20Font 13 Tf 0.1 0.2 0.3 rg % untouched /F#20Font 99 Tf\n".to_vec())));
    let alignment = doc.add(Object::Int(2));
    let widget = doc.add(dict(&[
        (b"Type", Object::name("Annot")),
        (b"Subtype", Object::name("Widget")),
        (b"FT", Object::name("Tx")),
        (b"T", Object::String(PdfString::text(name))),
        (b"V", Object::String(PdfString::text("Original"))),
        (b"Rect", Object::Array([10, 10, 210, 40].map(Object::Int).to_vec())),
        (b"OriginalFieldMetadata", Object::String(PdfString::text("retained"))),
    ]));
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), Vec::new())));
    let page = doc.add(dict(&[
        (b"Type", Object::name("Page")),
        (b"Parent", Object::Ref(pages)),
        (b"MediaBox", Object::Array([0, 0, 300, 300].map(Object::Int).to_vec())),
        (b"Contents", Object::Ref(contents)),
        (b"Annots", Object::Array(vec![Object::Ref(widget)])),
    ]));
    doc.update_dict(widget, |d| d.set(b"P".to_vec(), Object::Ref(page))).unwrap();
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    let mut form = dict(&[(b"Fields", Object::Array(vec![Object::Ref(widget)])), (b"DR", Object::Ref(dr))]);
    if catalog_defaults {
        form.set(b"DA".to_vec(), Object::Ref(da));
        form.set(b"Q".to_vec(), Object::Ref(alignment));
    } else {
        doc.update_dict(widget, |d| {
            d.set(b"DA".to_vec(), Object::Ref(da));
            d.set(b"Q".to_vec(), Object::Ref(alignment));
        })
        .unwrap();
    }
    let form = doc.add(form);
    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Ref(form))).unwrap();
    set_value(&mut doc, name, &FieldValue::Text("Original".into())).unwrap();
    reopen(&doc)
}

fn bytes(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}
fn reopen(doc: &Document) -> Document {
    Document::open(Arc::new(bytes(doc))).unwrap()
}
fn form(doc: &Document) -> Dict {
    pdfcraft_forms::acroform(doc).unwrap()
}
fn resource_fonts(doc: &Document) -> Dict {
    let form = form(doc);
    let dr = doc.resolve(form.get(b"DR").unwrap());
    doc.resolve(dr.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone()
}

fn assert_filled(doc: &Document, name: &str, expected_font: &str, count: usize) {
    let field = fields(doc).into_iter().find(|f| f.name == name).unwrap();
    assert_eq!(field.value, ["Changed"]);
    assert_eq!(field.quadding, 2);
    assert_eq!(field.widgets.len(), count);
    assert_eq!(doc.get(field.obj).as_dict().unwrap().get(b"OriginalFieldMetadata").unwrap().as_string().unwrap().bytes, b"retained");
    let da = pdfcraft_content::default_appearance::parse_default_appearance(field.da.as_bytes()).unwrap();
    assert_eq!(da.size, Some(13.0));
    assert_eq!(da.color, Some(vec![0.1, 0.2, 0.3]));
    for widget in field.widgets {
        let object = doc.get(widget.obj);
        let ap = doc.resolve(object.as_dict().unwrap().get(b"AP").unwrap());
        let appearance = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
        let stream = appearance.as_stream().unwrap();
        let content = stream.decoded().unwrap();
        let parsed = pdfcraft_content::parse(&content);
        assert_eq!(parsed.skipped, 0);
        let tf = parsed.ops.iter().find(|op| op.is("Tf")).unwrap();
        assert_eq!(tf.num(1), Some(13.0));
        let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        let font = doc.resolve(fonts.as_dict().unwrap().get(tf.name(0).unwrap()).unwrap());
        assert_eq!(font.as_dict().unwrap().name(b"BaseFont"), Some(expected_font.as_bytes()));
        assert_eq!(font.as_dict().unwrap().get(b"OriginalFontMetadata").unwrap().as_string().unwrap().to_text(), expected_font);
        assert!(parsed.ops.iter().any(|op| op.is("rg") && op.operands.iter().map(|o| o.as_f64().unwrap()).collect::<Vec<_>>() == [0.1, 0.2, 0.3]));
        assert!(parsed.ops.iter().any(|op| op.is("Tm") && op.num(4).unwrap() > 100.0), "right-aligned saved text retains its placement");
        assert!(parsed.ops.iter().any(|op| op.is("Tj") && op.operands[0].as_string().unwrap().bytes == b"Changed"));
    }
}

#[test]
fn extracted_single_and_repeated_widgets_keep_font_and_effective_catalog_defaults() {
    for catalog_defaults in [false, true] {
        let source = fixture("entry", "Courier", catalog_defaults);
        let original = bytes(&source);
        for selected in [vec![0], vec![0, 0]] {
            let mut copied = reopen(&extract_pages(&source, &selected).unwrap());
            let dr = copied.resolve(form(&copied).get(b"DR").unwrap());
            assert_eq!(dr.as_dict().unwrap().get(b"OriginalResourceMetadata").unwrap().as_string().unwrap().bytes, b"preserved");
            set_value(&mut copied, "entry", &FieldValue::Text("Changed".into())).unwrap();
            assert_filled(&reopen(&copied), "entry", "Courier", selected.len());
            assert_eq!(bytes(&source), original);
        }
    }
}

#[test]
fn inserting_and_combining_conflicting_font_aliases_preserves_both_field_bindings() {
    let source = fixture("imported", "Courier", true);
    let source_before = bytes(&source);
    let destination = fixture("existing", "Times-Roman", true);
    let destination_before = bytes(&destination);
    let original_form = form(&destination);
    let original_da = destination.resolve(original_form.get(b"DA").unwrap());
    let original_q = destination.resolve(original_form.get(b"Q").unwrap());
    for combine in [false, true] {
        let mut out = if combine {
            combine_selected(&[("Destination", &destination, None), ("Source", &source, Some(&[0, 0]))]).unwrap()
        } else {
            let mut out = destination.clone();
            import_pages(&mut out, &source, &[0, 0], 1).unwrap();
            assert_eq!(out.resolve(form(&out).get(b"DA").unwrap()), original_da);
            assert_eq!(out.resolve(form(&out).get(b"Q").unwrap()), original_q);
            out
        };
        let out_fonts = resource_fonts(&out);
        assert_eq!(out_fonts.len(), 2);
        let first = out.resolve(out_fonts.get(b"F Font").unwrap());
        let second = out.resolve(out_fonts.get(b"F Font_").unwrap());
        assert_eq!(first.as_dict().unwrap().name(b"BaseFont"), Some(b"Times-Roman".as_slice()));
        assert_eq!(second.as_dict().unwrap().name(b"BaseFont"), Some(b"Courier".as_slice()));
        let field = fields(&out).into_iter().find(|f| f.name == "imported").unwrap();
        assert!(field.da.starts_with("/F#20Font_ 13 Tf"));
        assert!(field.da.ends_with("% untouched /F#20Font 99 Tf\n"), "comment bytes are not font references");
        set_value(&mut out, "existing", &FieldValue::Text("Changed".into())).unwrap();
        set_value(&mut out, "imported", &FieldValue::Text("Changed".into())).unwrap();
        let saved = reopen(&out);
        assert_filled(&saved, "existing", "Times-Roman", 1);
        assert_filled(&saved, "imported", "Courier", 2);
        assert_eq!(bytes(&source), source_before);
        assert_eq!(bytes(&destination), destination_before);
    }
}

#[test]
fn unrepresentable_form_resource_and_da_conflicts_refuse_without_mutating_destination() {
    let destination = fixture("existing", "Times-Roman", true);
    for invalid in ["resource", "damaged-da", "missing-font"] {
        let mut source = fixture("imported", "Courier", true);
        let af = form(&source);
        if invalid == "damaged-da" {
            let da = af.reference(b"DA").unwrap();
            source.set(da, Object::String(PdfString::literal(b"/F#20Font 13 Tf (unfinished".to_vec())));
        } else if invalid == "missing-font" {
            let dr = source.resolve(af.get(b"DR").unwrap());
            let fonts = dr.as_dict().unwrap().reference(b"Font").unwrap();
            source
                .update_dict(fonts, |d| {
                    d.remove(b"F Font");
                })
                .unwrap();
        } else {
            let dr = af.reference(b"DR").unwrap();
            source.update_dict(dr, |d| d.set(b"OriginalResourceMetadata".to_vec(), Object::String(PdfString::text("conflict")))).unwrap();
        }
        let source_before = bytes(&source);
        let mut out = destination.clone();
        let before = bytes(&out);
        let root_before = out.get(out.root().unwrap());
        let field_before: Vec<ObjRef> = fields(&out).iter().map(|f| f.obj).collect();
        let error = import_pages(&mut out, &source, &[0, 0], 1).unwrap_err().to_string();
        let expected = match invalid {
            "damaged-da" => "DA cannot be remapped",
            "missing-font" => "DA font has no source resource",
            _ => "conflicting imported form resource",
        };
        assert!(error.contains(expected), "{error}");
        assert_eq!(bytes(&out), before);
        assert_eq!(out.get(out.root().unwrap()), root_before);
        assert_eq!(fields(&out).iter().map(|f| f.obj).collect::<Vec<_>>(), field_before);
        assert_eq!(bytes(&source), source_before);
    }
}

#[test]
fn imported_aliases_do_not_steal_other_source_fonts_or_destination_alignment() {
    let mut source = fixture("imported", "Courier", true);
    let af = form(&source);
    let dr = source.resolve(af.get(b"DR").unwrap());
    let font_dict = dr.as_dict().unwrap().reference(b"Font").unwrap();
    let other = source.add(dict(&[(b"Type", Object::name("Font")), (b"Subtype", Object::name("Type1")), (b"BaseFont", Object::name("Helvetica"))]));
    source.update_dict(font_dict, |d| d.set(b"F Font_".to_vec(), Object::Ref(other))).unwrap();
    // Q is optional and defaults to left alignment in the source.
    let source_form = source.get(source.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
    source
        .update_dict(source_form, |d| {
            d.remove(b"Q");
        })
        .unwrap();
    let before = bytes(&source);
    let mut out = fixture("existing", "Times-Roman", true);
    import_pages(&mut out, &source, &[0], 1).unwrap();
    let fonts = resource_fonts(&out);
    assert_eq!(fonts.len(), 3);
    assert_eq!(out.resolve(fonts.get(b"F Font_").unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(b"Helvetica".as_slice()));
    assert_eq!(out.resolve(fonts.get(b"F Font__").unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(b"Courier".as_slice()));
    set_value(&mut out, "imported", &FieldValue::Text("Changed".into())).unwrap();
    let saved = reopen(&out);
    let field = fields(&saved).into_iter().find(|f| f.name == "imported").unwrap();
    assert_eq!(field.quadding, 0);
    assert!(field.da.starts_with("/F#20Font__ 13 Tf"));
    let widget = saved.get(field.widgets[0].obj);
    let ap = saved.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
    let normal = saved.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let parsed = pdfcraft_content::parse(&normal.as_stream().unwrap().decoded().unwrap());
    assert!(
        parsed.ops.iter().any(|op| op.is("Tm") && op.num(4).is_some_and(|x| x < 10.0)),
        "source left alignment must not become destination right alignment"
    );
    assert_eq!(bytes(&source), before);
}
