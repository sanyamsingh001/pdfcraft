use super::*;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use std::sync::Arc;

// Contributor-original blank page and technical CMaps. CID advances are explicit;
// these tests qualify encoded bytes and geometry, not font outlines or rendering.
fn original(mixed: bool, two_bytes: bool) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let a = if two_bytes { "8141" } else { "41" };
    let b = if mixed || two_bytes { "8142" } else { "42" };
    let spaces = if mixed {
        "2 begincodespacerange <00> <7F> <8100> <81FF> endcodespacerange"
    } else if two_bytes {
        "1 begincodespacerange <8100> <81FF> endcodespacerange"
    } else {
        "1 begincodespacerange <00> <7F> endcodespacerange"
    };
    let encoding = format!("/WMode 0 def {spaces} 2 begincidchar <{a}> 7 <{b}> 8 endcidchar");
    let encoding = doc.add(Object::Stream(Stream::from_raw(Dict::new(), encoding.into_bytes())));
    let unicode = format!("{spaces} 2 beginbfchar <{a}> <0041> <{b}> <0042> endbfchar");
    let unicode = doc.add(Object::Stream(Stream::from_raw(Dict::new(), unicode.into_bytes())));
    let mut system = Dict::new();
    system.set(b"Registry".to_vec(), PdfString::literal(b"Original".to_vec()));
    system.set(b"Ordering".to_vec(), PdfString::literal(b"Test".to_vec()));
    system.set(b"Supplement".to_vec(), Object::Int(0));
    let mut descendant = Dict::new();
    descendant.set(b"Type".to_vec(), Object::name("Font"));
    descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    descendant.set(b"BaseFont".to_vec(), Object::name("OriginalCID"));
    descendant.set(b"CIDSystemInfo".to_vec(), Object::Dict(system));
    descendant.set(b"DW".to_vec(), Object::Int(600));
    descendant.set(b"W".to_vec(), Object::Array(vec![Object::Int(7), Object::Array(vec![Object::Int(600), Object::Int(800)])]));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type0"));
    font.set(b"BaseFont".to_vec(), Object::name("OriginalCID"));
    font.set(b"Encoding".to_vec(), Object::Ref(encoding));
    font.set(b"ToUnicode".to_vec(), Object::Ref(unicode));
    font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Ref(doc.add(descendant))]));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(doc.add(font)));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let content = format!("BT /F1 10 Tf 1 0 0 1 72 700 Tm <{a}> Tj ET");
    let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 600, 800].into_iter().map(Object::Int).collect()));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Ref(content));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    saved(&doc)
}

fn saved(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn shown(doc: &Document) -> Vec<Vec<u8>> {
    let page = page_dict(doc, 0).unwrap();
    Content::read(doc, &page.dict).unwrap().ops.iter().filter(|op| op.is("Tj")).map(|op| op.operands[0].as_string().unwrap().bytes.clone()).collect()
}

#[test]
fn line_replacement_refuses_mixed_length_source_reuse_without_changing_saved_bytes() {
    for replacement in ["A", "AB"] {
        let mut doc = original(true, false);
        let lines = text_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "A");
        assert_eq!(shown(&doc), [b"A".to_vec()]);
        assert!((lines[0].rect[2] - lines[0].rect[0] - 6.0).abs() < 1e-8);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        let error = replace_line(&mut doc, 0, 0, replacement).unwrap_err();
        assert!(error.to_string().contains("mixed-length"), "{error}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        let reopened = saved(&doc);
        assert_eq!(shown(&reopened), [b"A".to_vec()]);
        assert_eq!(text_lines(&reopened, 0).unwrap()[0].text, "A");
    }
}

#[test]
fn line_replacement_reuses_fixed_length_cmaps_with_exact_saved_codes_and_cid_advances() {
    for two_bytes in [false, true] {
        let mut doc = original(false, two_bytes);
        assert_eq!(text_lines(&doc, 0).unwrap()[0].text, "A");
        assert_eq!(replace_line(&mut doc, 0, 0, "BA").unwrap().substituted, None);
        let reopened = saved(&doc);
        let expected = if two_bytes { vec![0x81, 0x42, 0x81, 0x41] } else { b"BA".to_vec() };
        assert_eq!(shown(&reopened), [expected]);
        let lines = text_lines(&reopened, 0).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "BA");
        assert_eq!(lines[0].font_name, b"F1");
        assert!((lines[0].rect[0] - 72.0).abs() < 1e-8);
        assert!((lines[0].rect[2] - 86.0).abs() < 1e-8, "CID 8 and 7 advance 8 + 6 points");
    }
}
