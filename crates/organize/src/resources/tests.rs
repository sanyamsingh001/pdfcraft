use std::sync::Arc;

use pdfcraft_cos::{ObjRef, SaveOptions, Stream, write_full};

use super::*;
use crate::{SplitBy, extract_pages, pages, split};

fn dictionary(entries: &[(&[u8], Object)]) -> Dict {
    entries.iter().map(|(k, v)| (k.to_vec(), v.clone())).collect()
}

struct Fixture {
    doc: Document,
    resources: ObjRef,
    images: [ObjRef; 2],
    contents: [ObjRef; 2],
    pages: [ObjRef; 2],
}

fn fixture(inherited: bool) -> Fixture {
    let mut doc = Document::new_empty();
    let image_dict = dictionary(&[
        (b"Type", Object::name("XObject")),
        (b"Subtype", Object::name("Image")),
        (b"Width", Object::Int(256)),
        (b"Height", Object::Int(256)),
        (b"ColorSpace", Object::name("DeviceRGB")),
        (b"BitsPerComponent", Object::Int(8)),
    ]);
    let images = [0u8, 1].map(|seed| {
        let data = (0..256 * 256 * 3).map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed)).collect();
        doc.add(Object::Stream(Stream::from_raw(image_dict.clone(), data)))
    });
    let font = doc.add(dictionary(&[(b"Type", Object::name("Font")), (b"Subtype", Object::name("Type1")), (b"BaseFont", Object::name("Helvetica"))]));
    let xobjects = doc.add(dictionary(&[(b"Im1", Object::Ref(images[0])), (b"Im2", Object::Ref(images[1]))]));
    let resources = doc.add(dictionary(&[(b"XObject", Object::Ref(xobjects)), (b"Font", Object::Dict(dictionary(&[(b"F1", Object::Ref(font))])))]));
    let contents = [1, 2].map(|i| doc.add(Object::Stream(Stream::from_raw(Dict::new(), format!("q 256 0 0 256 0 0 cm /Im{i} Do Q\n").into_bytes()))));
    let root = crate::pages_root(&doc).unwrap();
    let pages = contents.map(|content| {
        let mut page = dictionary(&[(b"Type", Object::name("Page")), (b"Parent", Object::Ref(root)), (b"Contents", Object::Ref(content))]);
        if !inherited {
            page.set(b"Resources".to_vec(), Object::Ref(resources));
        }
        doc.add(page)
    });
    let mut tree = dictionary(&[
        (b"Type", Object::name("Pages")),
        (b"Kids", Object::Array(pages.map(Object::Ref).to_vec())),
        (b"Count", Object::Int(2)),
        (b"MediaBox", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
    ]);
    if inherited {
        tree.set(b"Resources".to_vec(), Object::Ref(resources));
    }
    doc.set(root, Object::Dict(tree));
    Fixture { doc, resources, images, contents, pages }
}

fn bytes(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn resources(doc: &Document, page: usize) -> Dict {
    let inherited = crate::walk(doc).unwrap()[page].1.clone();
    doc.resolve(inherited.get(b"Resources").unwrap()).as_dict().unwrap().clone()
}

fn xobjects(doc: &Document, page: usize) -> Dict {
    doc.resolve(resources(doc, page).get(b"XObject").unwrap()).as_dict().unwrap().clone()
}

fn stream_data(object: &Object) -> Arc<Vec<u8>> {
    let Object::Stream(stream) = object else { panic!("expected a stream") };
    stream.raw.clone()
}

fn image_count(doc: &Document) -> usize {
    doc.object_numbers()
        .iter()
        .filter(|n| matches!(doc.get(ObjRef::new(**n, doc.generation(**n))).as_ref(), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .count()
}

fn content(f: &mut Fixture, data: &[u8]) {
    f.doc.set(f.contents[0], Object::Stream(Stream::from_raw(Dict::new(), data.to_vec())));
}

fn add_resource(f: &mut Fixture, category: &[u8], name: &[u8], value: Object) {
    let mut res = f.doc.get(f.resources).as_dict().unwrap().clone();
    let mut entries = res.get(category).map(|o| f.doc.resolve(o).as_dict().unwrap().clone()).unwrap_or_default();
    entries.set(name.to_vec(), value);
    res.set(category.to_vec(), Object::Dict(entries));
    f.doc.set(f.resources, Object::Dict(res));
}

#[test]
fn split_shared_images_shrinks_outputs_without_mutating_source() {
    for inherited in [false, true] {
        let f = fixture(inherited);
        let original = bytes(&f.doc);
        let parts = split(&f.doc, &SplitBy::PageCount(1)).unwrap();
        assert_eq!(parts.len(), 2);
        for (i, part) in parts.iter().enumerate() {
            let output = bytes(part);
            assert!(output.len() * 10 < original.len() * 6, "one image instead of both: {} / {}", output.len(), original.len());
            let independent = hayro_syntax::Pdf::new(output.clone()).unwrap();
            assert_eq!(independent.pages().len(), 1);
            let reopened = Document::open(Arc::new(output)).unwrap();
            assert_eq!(image_count(&reopened), 1);
            let images = xobjects(&reopened, 0);
            assert_eq!(images.len(), 1);
            let image = reopened.resolve(images.get(format!("Im{}", i + 1).as_bytes()).unwrap());
            assert_eq!(stream_data(&image), stream_data(&f.doc.get(f.images[i])));
            let Object::Stream(image) = image.as_ref() else { panic!("image stream") };
            assert_eq!(image.dict.int(b"Width"), Some(256));
            assert_eq!(image.dict.int(b"Height"), Some(256));
            assert_eq!(image.dict.name(b"ColorSpace"), Some(b"DeviceRGB".as_slice()));
            assert_eq!(image.dict.int(b"BitsPerComponent"), Some(8));
            let page = reopened.get(pages(&reopened).unwrap()[0].obj);
            let copied = reopened.resolve(page.as_dict().unwrap().get(b"Contents").unwrap());
            assert_eq!(stream_data(&copied), stream_data(&f.doc.get(f.contents[i])));
        }
        assert_eq!(bytes(&f.doc), original);
        assert_eq!(xobjects(&f.doc, 0).len(), 2);
    }
}

#[test]
fn selected_pages_keep_independent_resource_sets_and_duplicate_pages() {
    let f = fixture(true);
    let out = extract_pages(&f.doc, &[1, 0, 1]).unwrap();
    assert_eq!(image_count(&out), 2);
    for (page, expected) in [b"Im2", b"Im1", b"Im2"].iter().enumerate() {
        let images = xobjects(&out, page);
        assert_eq!(images.len(), 1);
        assert!(images.contains(*expected));
    }
}

#[test]
fn text_names_comments_and_stream_array_boundaries_are_analyzed() {
    let mut f = fixture(true);
    let first = f.doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q /Im#31".to_vec())));
    let second = f
        .doc
        .add(Object::Stream(Stream::flate(Dict::new(), b" Do Q % /Im2 Do\nBT /F1 12 Tf 10 10 Td (literal /Im2 Do) Tj [(more) 2 (text)] TJ ET\n")));
    let mut page = f.doc.get(f.pages[0]).as_dict().unwrap().clone();
    page.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(first), Object::Ref(second)]));
    f.doc.set(f.pages[0], Object::Dict(page));
    let out = extract_pages(&f.doc, &[0]).unwrap();
    assert_eq!(image_count(&out), 1);
    assert!(xobjects(&out, 0).contains(b"Im1"));
}

#[test]
fn uncertain_content_keeps_all_images() {
    for data in [
        b"/Im1 Do ]".as_slice(),
        b"/Im1 Do /Im2",
        b"/Im1 /Im2 Do",
        b"/Im1 Do extension",
        b"q /Im1 Do",
        b"Q /Im1 Do",
        b"/Missing Do",
        b"/Im1 Do BI /W 1 /H 1 /BPC 8 /CS /G ID x EI",
        b"/Im1 Do (unterminated",
        b"/Im1 Do /GS1 gs",
        b"/Im1 Do /Pattern cs /P1 scn",
        b"/Im1 Do /Span BMC EMC",
    ] {
        let mut f = fixture(true);
        content(&mut f, data);
        assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2, "{data:?}");
    }
}

#[test]
fn nested_forms_type3_patterns_and_annotation_appearances_keep_dependencies() {
    for kind in ["form", "type3", "pattern", "annot"] {
        let mut f = fixture(true);
        let form = f.doc.add(Object::Stream(Stream::from_raw(
            dictionary(&[
                (b"Type", Object::name("XObject")),
                (b"Subtype", Object::name("Form")),
                (b"BBox", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
            ]),
            b"/Im2 Do".to_vec(),
        )));
        match kind {
            "form" => {
                add_resource(&mut f, b"XObject", b"Inner", Object::Ref(form));
                let outer = f.doc.add(Object::Stream(Stream::from_raw(
                    dictionary(&[
                        (b"Type", Object::name("XObject")),
                        (b"Subtype", Object::name("Form")),
                        (b"BBox", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
                    ]),
                    b"/Inner Do".to_vec(),
                )));
                add_resource(&mut f, b"XObject", b"Fm", Object::Ref(outer));
                content(&mut f, b"/Im1 Do /Fm Do");
            }
            "type3" => {
                let glyph = f.doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"256 0 d0 /Im2 Do".to_vec())));
                let font = dictionary(&[
                    (b"Type", Object::name("Font")),
                    (b"Subtype", Object::name("Type3")),
                    (b"FontBBox", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
                    (
                        b"FontMatrix",
                        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
                    ),
                    (b"FirstChar", Object::Int(65)),
                    (b"LastChar", Object::Int(65)),
                    (b"Widths", Object::Array(vec![Object::Int(256)])),
                    (b"Encoding", Object::Dict(dictionary(&[(b"Differences", Object::Array(vec![Object::Int(65), Object::name("A")]))]))),
                    (b"Resources", Object::Ref(f.resources)),
                    (b"CharProcs", Object::Dict(dictionary(&[(b"A", Object::Ref(glyph))]))),
                ]);
                add_resource(&mut f, b"Font", b"F1", Object::Dict(font));
                content(&mut f, b"/Im1 Do BT /F1 12 Tf (A) Tj ET");
            }
            "pattern" => {
                let pattern = f.doc.add(Object::Stream(Stream::from_raw(
                    dictionary(&[
                        (b"Type", Object::name("Pattern")),
                        (b"PatternType", Object::Int(1)),
                        (b"PaintType", Object::Int(1)),
                        (b"TilingType", Object::Int(1)),
                        (b"BBox", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
                        (b"XStep", Object::Int(256)),
                        (b"YStep", Object::Int(256)),
                        (b"Resources", Object::Ref(f.resources)),
                    ]),
                    b"/Im2 Do".to_vec(),
                )));
                add_resource(&mut f, b"Pattern", b"P1", Object::Ref(pattern));
                content(&mut f, b"/Im1 Do /Pattern cs /P1 scn 0 0 256 256 re f");
            }
            "annot" => {
                let annot = dictionary(&[
                    (b"Type", Object::name("Annot")),
                    (b"Subtype", Object::name("Stamp")),
                    (b"Rect", Object::Array([0, 0, 256, 256].map(Object::Int).to_vec())),
                    (b"AP", Object::Dict(dictionary(&[(b"N", Object::Ref(form))]))),
                ]);
                let mut page = f.doc.get(f.pages[0]).as_dict().unwrap().clone();
                page.set(b"Annots".to_vec(), Object::Array(vec![Object::Dict(annot)]));
                f.doc.set(f.pages[0], Object::Dict(page));
            }
            _ => panic!("unknown resource fixture kind: {kind}"),
        }
        assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2, "{kind}");
    }
}

#[test]
fn unknown_resources_and_xobjects_are_preserved() {
    let mut f = fixture(true);
    let unknown = Object::Dict(dictionary(&[(b"Opaque", Object::Int(42))]));
    add_resource(&mut f, b"XObject", b"Unknown", unknown.clone());
    let out = extract_pages(&f.doc, &[0]).unwrap();
    assert_eq!(xobjects(&out, 0).get(b"Unknown"), Some(&unknown));
    assert_eq!(image_count(&out), 1);
    add_resource(&mut f, b"Extension", b"Unknown", unknown);
    let out = extract_pages(&f.doc, &[0]).unwrap();
    assert_eq!(image_count(&out), 2);
    assert_eq!(resources(&out, 0).get(b"Extension"), resources(&f.doc, 0).get(b"Extension"));
}

#[test]
fn broken_filters_and_decode_limits_preserve_original_resources() {
    for filter in
        [Object::name("UnsupportedFilter"), Object::name("FlateDecode"), Object::Array(vec![Object::Int(9)]), Object::Ref(ObjRef::new(999, 0))]
    {
        let mut f = fixture(true);
        f.doc.set(f.contents[0], Object::Stream(Stream::from_raw(dictionary(&[(b"Filter", filter)]), b"/Im1 Do".to_vec())));
        assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2);
    }
    let mut f = fixture(true);
    let params = dictionary(&[(b"Predictor", Object::Ref(ObjRef::new(999, 0)))]);
    f.doc.set(f.contents[0], Object::Stream(Stream::from_raw(dictionary(&[(b"DecodeParms", Object::Dict(params))]), b"/Im1 Do".to_vec())));
    assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2);
    let raw = Stream::from_raw(Dict::new(), b"12345".to_vec());
    let compressed = Stream::flate(Dict::new(), b"12345");
    for stream in [raw, compressed] {
        assert!(stream.decoded_strict_within(4).is_err());
        assert_eq!(stream.decoded_strict_within(5).unwrap(), b"12345");
    }
    let mut f = fixture(true);
    content(&mut f, &vec![b' '; MAX_CONTENT + 1]);
    assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2);
}

#[test]
fn ambiguous_stream_boundaries_keep_all_resources() {
    for (first, second) in [
        // Raw concatenation hides the image in a string; a separating space closes the
        // first string and paints the image before the later malformed delimiter.
        (&b"BT /F1 12 Tf (prefix\\"[..], &b") Tj ET /Im2 Do BT /F1 12 Tf (suffix)) Tj ET"[..]),
        // A newline terminates the comment and reveals a different image dependency.
        (&b"/Im1 Do % comment"[..], &b"/Im2 Do"[..]),
        // Splitting inside an escaped resource name is not a lexical-token boundary.
        (&b"/Im#3"[..], &b"1 Do"[..]),
    ] {
        let mut f = fixture(true);
        let refs: Vec<_> =
            [first, second].iter().map(|data| Object::Ref(f.doc.add(Object::Stream(Stream::from_raw(Dict::new(), data.to_vec()))))).collect();
        let mut page = f.doc.get(f.pages[0]).as_dict().unwrap().clone();
        page.set(b"Contents".to_vec(), Object::Array(refs));
        f.doc.set(f.pages[0], Object::Dict(page));
        assert_eq!(image_count(&extract_pages(&f.doc, &[0]).unwrap()), 2, "ambiguous boundary must not remove resources");
    }
}
