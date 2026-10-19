//! Contributor-original image/form drawings and ordinary resource-name collisions.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

use crate::added::{AddedImage, Content, add_content};
use crate::{Background, ImageChange, MarkSource, Watermark, add_background, add_watermark, change_image, page_images};

fn dict(entries: &[(&[u8], Object)]) -> Dict {
    entries.iter().map(|(key, value)| (key.to_vec(), value.clone())).collect()
}

struct Fixture {
    doc: Document,
    page: ObjRef,
    legacy: ObjRef,
    old: ObjRef,
    target: ObjRef,
    preferred: String,
    form: bool,
}

fn fixture(prefix: &str, inherited: bool, form: bool) -> Fixture {
    let mut doc = Document::new_empty();
    let root = doc.root().unwrap();
    let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let image = dict(&[
        (b"Type", Object::name("XObject")),
        (b"Subtype", Object::name("Image")),
        (b"Width", Object::Int(1)),
        (b"Height", Object::Int(1)),
        (b"ColorSpace", Object::name("DeviceGray")),
        (b"BitsPerComponent", Object::Int(8)),
    ]);
    let old = doc.add(Object::Stream(Stream::from_raw(image.clone(), vec![32])));
    let target = if form {
        doc.add(Object::Stream(Stream::from_raw(
            dict(&[
                (b"Type", Object::name("XObject")),
                (b"Subtype", Object::name("Form")),
                (b"BBox", Object::Array([0, 0, 10, 10].map(Object::Int).to_vec())),
                (b"Resources", Object::Dict(Dict::new())),
            ]),
            b"0 g 0 0 10 10 re f".to_vec(),
        )))
    } else {
        doc.add(Object::Stream(Stream::from_raw(image, vec![224])))
    };
    let preferred = format!("{prefix}{}", target.num);
    let xobjects = doc.add(Object::Dict(dict(&[
        (b"Selected", Object::Ref(old)),
        (b"Available", Object::Ref(target)),
        (preferred.as_bytes(), Object::Ref(old)),
        (format!("{preferred}_").as_bytes(), Object::Ref(old)),
    ])));
    let state = doc.add(Object::Dict(dict(&[
        (b"Type", Object::name("ExtGState")),
        (b"CA", Object::Real(0.9)),
        (b"ca", Object::Real(0.9)),
        (b"Unknown", Object::Int(73)),
    ])));
    let states = doc.add(Object::Dict(dict(&[(b"PCGS50", Object::Ref(state)), (b"PCGS50_", Object::Ref(state))])));
    let resources = doc.add(Object::Dict(dict(&[
        (b"XObject", Object::Ref(xobjects)),
        (b"ExtGState", Object::Ref(states)),
        (b"UnknownResource", Object::Int(17)),
    ])));
    let selected = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 20 0 0 20 10 10 cm /Selected Do Q".to_vec())));
    let legacy = doc.add(Object::Stream(Stream::from_raw(
        Dict::new(),
        format!("/PCGS50 gs q 20 0 0 20 50 10 cm /{preferred} Do Q q 20 0 0 20 90 10 cm /{preferred}_ Do Q % untouched original bytes\n")
            .into_bytes(),
    )));
    let mut page = dict(&[
        (b"Type", Object::name("Page")),
        (b"Parent", Object::Ref(pages)),
        (b"Contents", Object::Array(vec![Object::Ref(selected), Object::Ref(legacy)])),
        (b"MediaBox", Object::Array([0, 0, 300, 400].map(Object::Int).to_vec())),
    ]);
    if !inherited {
        page.set(b"Resources".to_vec(), Object::Ref(resources));
    }
    let page = doc.add(Object::Dict(page));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
        if inherited {
            d.set(b"Resources".to_vec(), Object::Ref(resources));
        }
    })
    .unwrap();
    Fixture { doc: reopen(&doc), page, legacy, old, target, preferred, form }
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions { object_streams: false, ..Default::default() }).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn resources(doc: &Document) -> Dict {
    let page = &pdfcraft_model::pages(doc)[0];
    doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone()
}

fn category(doc: &Document, key: &[u8]) -> Dict {
    doc.resolve(resources(doc).get(key).unwrap()).as_dict().unwrap().clone()
}

fn mark_aliases(doc: &Document, tag: &str, op: &str) -> Vec<String> {
    let page = &pdfcraft_model::pages(doc)[0];
    let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
    let objects = match &*contents {
        Object::Array(items) => items.clone(),
        _ => vec![contents.as_ref().clone()],
    };
    let mut aliases = Vec::new();
    for object in objects {
        let object = doc.resolve(&object);
        let Object::Stream(stream) = &*object else { continue };
        if stream.dict.name(b"PCMark") != Some(tag.as_bytes()) {
            continue;
        }
        let parsed = pdfcraft_content::parse(&stream.decoded_strict().unwrap());
        assert_eq!(parsed.skipped, 0);
        aliases.extend(parsed.ops.iter().filter(|o| o.is(op)).map(|o| String::from_utf8(o.name(0).unwrap().to_vec()).unwrap()));
    }
    aliases
}

fn preserves_originals(f: &Fixture, doc: &Document, original_legacy: &Object, original_resources: &Dict) {
    assert_eq!(&*doc.get(f.legacy), original_legacy, "untouched legacy content is byte-identical");
    assert_eq!(resources(doc).get(b"UnknownResource"), Some(&Object::Int(17)));
    for key in [b"XObject".as_slice(), b"ExtGState"] {
        let old = f.doc.resolve(original_resources.get(key).unwrap());
        let current = category(doc, key);
        for (name, value) in old.as_dict().unwrap().iter() {
            assert_eq!(current.get(name), Some(value), "reserved original resource was overwritten");
        }
    }
    assert_eq!(doc.get(f.old), f.doc.get(f.old));
}

#[test]
fn image_add_and_replace_reserve_imported_names_and_reuse_the_same_target() {
    for inherited in [false, true] {
        for replace in [false, true] {
            let mut f = fixture("PCImg", inherited, false);
            let original_legacy = f.doc.get(f.legacy);
            let original_resources = resources(&f.doc);
            if replace {
                let target = f.target;
                change_image(&mut f.doc, 0, 0, &ImageChange::Replace(target)).unwrap();
            } else {
                let image = Content::Image(AddedImage::new([10.0, 100.0, 30.0, 120.0], f.target));
                add_content(&mut f.doc, 0, &image).unwrap();
            }
            let entries = category(&f.doc, b"XObject");
            let alias = format!("{}__", f.preferred);
            assert_eq!(entries.get(alias.as_bytes()), Some(&Object::Ref(f.target)));
            assert_eq!(entries.len(), 5);
            let image = Content::Image(AddedImage::new([50.0, 100.0, 70.0, 120.0], f.target));
            add_content(&mut f.doc, 0, &image).unwrap();
            assert_eq!(category(&f.doc, b"XObject").len(), 5, "exactly identical targets reuse the reserved alias");
            let doc = reopen(&f.doc);
            preserves_originals(&f, &doc, &original_legacy, &original_resources);
            let images = page_images(&doc, 0).unwrap();
            assert_eq!(images.iter().filter(|i| i.object == Some(f.target)).count(), 2);
            assert_eq!(images.iter().filter(|i| i.object == Some(f.old)).count(), if replace { 2 } else { 3 });
            assert!(images.iter().filter(|i| i.object == Some(f.target)).all(|i| i.name == alias));
        }
    }
}

#[test]
fn picture_marks_reserve_names_for_images_and_forms_without_rewriting_legacy_content() {
    for inherited in [false, true] {
        for form in [false, true] {
            let mut f = fixture("PCPic", inherited, form);
            let original_legacy = f.doc.get(f.legacy);
            let original_resources = resources(&f.doc);
            let source = MarkSource { xobject: f.target, size: if f.form { (10.0, 10.0) } else { (1.0, 1.0) }, image: !f.form };
            add_watermark(&mut f.doc, &[0, 0], &Watermark { source: Some(source), ..Default::default() }, false).unwrap();
            add_background(&mut f.doc, &[0], &Background { source: Some(source), opacity: 0.5, ..Default::default() }, false).unwrap();
            let doc = reopen(&f.doc);
            preserves_originals(&f, &doc, &original_legacy, &original_resources);
            let alias = format!("{}__", f.preferred);
            assert_eq!(mark_aliases(&doc, "Watermark", "Do"), vec![alias.clone(), alias.clone()]);
            assert_eq!(mark_aliases(&doc, "Background", "Do"), vec![alias.clone()]);
            assert_eq!(category(&doc, b"XObject").get(alias.as_bytes()), Some(&Object::Ref(f.target)));
            assert_eq!(category(&doc, b"XObject").len(), 5, "pending and later identical targets reuse one entry");
            assert_eq!(page_images(&doc, 0).unwrap().iter().filter(|i| i.object == Some(f.old)).count(), 3);
        }
    }
}

#[test]
fn nearby_opacities_keep_each_mark_value_and_the_original_graphics_state() {
    let mut f = fixture("PCImg", true, false);
    let original_legacy = f.doc.get(f.legacy);
    let original_resources = resources(&f.doc);
    add_watermark(&mut f.doc, &[0, 0], &Watermark { text: "First".into(), opacity: 0.501, ..Default::default() }, false).unwrap();
    add_watermark(&mut f.doc, &[0], &Watermark { text: "Second".into(), opacity: 0.504, ..Default::default() }, false).unwrap();
    add_background(&mut f.doc, &[0], &Background { opacity: 0.504, ..Default::default() }, false).unwrap();
    let doc = reopen(&f.doc);
    preserves_originals(&f, &doc, &original_legacy, &original_resources);
    let aliases = mark_aliases(&doc, "Watermark", "gs");
    assert_eq!(aliases, vec!["PCGS50__", "PCGS50__", "PCGS50___"]);
    assert_eq!(mark_aliases(&doc, "Background", "gs"), vec!["PCGS50___"]);
    let states = category(&doc, b"ExtGState");
    assert_eq!(states.len(), 4, "nearby opacities differ; exact repeat reuses its value");
    for (alias, value) in aliases.iter().zip([0.501, 0.501, 0.504]) {
        let state = doc.resolve(states.get(alias.as_bytes()).unwrap());
        assert_eq!(state.as_dict().unwrap().get(b"CA").unwrap().as_f64(), Some(value));
        assert_eq!(state.as_dict().unwrap().get(b"ca").unwrap().as_f64(), Some(value));
    }
    // An alias in another category has no bearing on ExtGState reservation.
    let mut f = fixture("PCImg", false, false);
    let mut res = resources(&f.doc);
    let mut images = category(&f.doc, b"XObject");
    images.set(b"PCGS60".to_vec(), Object::Ref(f.old));
    res.set(b"XObject".to_vec(), Object::Dict(images));
    let page = f.page;
    f.doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
    add_background(&mut f.doc, &[0], &Background { opacity: 0.6, ..Default::default() }, false).unwrap();
    assert_eq!(mark_aliases(&reopen(&f.doc), "Background", "gs"), vec!["PCGS60"]);
}
