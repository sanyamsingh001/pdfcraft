use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_incremental};

use super::*;

/// Three pages: an upright page with content that leaves the graphics state changed (an
/// unbalanced `cm`), a page rotated 90° and one with inherited, shared resources.
fn fixture() -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 600 800] /Resources 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents [7 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Font << /F1 8 0 R >> >>".into(),
        "<< /Length 37 >>\nstream\n2 0 0 2 0 0 cm BT /F1 9 Tf (Hi) Tj ET\nendstream".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

/// The decoded content streams of a page, in order.
fn streams(doc: &Document, page: usize) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[page];
    let c = p.dict.get(b"Contents").cloned();
    let list = match c.map(|c| doc.resolve(&c)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(_) => vec![p.dict.get(b"Contents").cloned().unwrap()],
        None => vec![],
    };
    list.iter().map(|o| String::from_utf8_lossy(&stream_bytes(doc, o).unwrap()).into_owned()).collect()
}

fn cx() -> Context {
    Context { date: (2026, 10, 1) }
}

#[test]
fn tokens_expand() {
    let c = cx();
    assert_eq!(expand("Page <<1>> of <<n>>", 3, 10, 0, &c), "Page 3 of 10");
    assert_eq!(expand("<<1 of n>> · <<Page 1>> · <<1/n>>", 2, 5, 0, &c), "2 of 5 · Page 2 · 2/5");
    assert_eq!(expand("<<m/d/yyyy>> <<yyyy-mm-dd>> <<mmmm d, yyyy>>", 1, 1, 0, &c), "10/1/2026 2026-10-01 October 1, 2026");
    assert_eq!(expand("<<Bates Number#6#100#ABC#-X>>", 1, 1, 104, &c), "ABC000104-X");
    assert_eq!(expand("keep <<unknown>> and <<unclosed", 1, 1, 0, &c), "keep <<unknown>> and <<unclosed");
    assert_eq!(bates_start("x <<Bates Number#6#100#A#B>>"), Some(100));
}

#[test]
fn header_and_footer_are_drawn_in_display_space_and_wrap_the_original_content() {
    let mut doc = fixture();
    let hf = HeaderFooter {
        text: ["Left".into(), String::new(), "<<Page 1 of n>>".into(), String::new(), "Confidential (draft)".into(), String::new()],
        ..HeaderFooter::default()
    };
    add_header_footer(&mut doc, &[0, 1, 2], &hf, false, &cx()).unwrap();
    let doc = reopen(&doc);
    let s0 = streams(&doc, 0);
    // q-wrapper, original, Q-wrapper, header/footer.
    assert_eq!(s0.len(), 4, "{s0:?}");
    assert_eq!((s0[0].as_str(), s0[2].as_str()), ("q %PdfCraft\n", "Q %PdfCraft\n"));
    let mark = &s0[3];
    assert!(mark.contains("/PCMark /HeaderFooter") && mark.contains("(Page 1 of 3) Tj") && mark.contains("(Confidential \\(draft\\)) Tj"), "{mark}");
    assert!(mark.contains("1 0 0 1 0 0 cm"), "upright page: identity");
    // The rotated page maps display space onto its rotated crop box.
    let s1 = streams(&doc, 1);
    assert!(s1.last().unwrap().contains("0 1 -1 0 600 0 cm") && s1.last().unwrap().contains("(Page 2 of 3)"), "{s1:?}");
    // A page without content gets just the mark; its inherited resources are copied, not changed.
    assert_eq!(streams(&doc, 2).len(), 1);
    let p2 = &pdfcraft_model::pages(&doc)[2];
    let res = doc.resolve(p2.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    assert!(fonts.as_dict().unwrap().contains(b"PCHelv") && fonts.as_dict().unwrap().contains(b"F1"));
    let shared = doc.get(pdfcraft_cos::ObjRef::new(6, 0));
    let shared_fonts = doc.resolve(shared.as_dict().unwrap().get(b"Font").unwrap());
    assert!(!shared_fonts.as_dict().unwrap().contains(b"PCHelv"), "the shared dictionary is untouched");
    assert_eq!(marks_present(&doc), [MarkKind::HeaderFooter]);
}

#[test]
fn replace_and_remove_restore_the_original_content() {
    let mut doc = fixture();
    let original = streams(&doc, 0);
    let mut hf = HeaderFooter::default();
    hf.text[1] = "First".into();
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    hf.text[1] = "Second".into();
    add_header_footer(&mut doc, &[0], &hf, true, &cx()).unwrap();
    let s = streams(&doc, 0);
    assert_eq!(s.iter().filter(|x| x.contains("/PCMark")).count(), 1, "replaced, not added");
    assert!(s.last().unwrap().contains("(Second)"));
    add_watermark(&mut doc, &[0], &Watermark { text: "DRAFT".into(), ..Watermark::default() }, false).unwrap();
    add_background(&mut doc, &[0], &Background { color: [1.0, 1.0, 0.9], opacity: 1.0, ..Background::default() }, false).unwrap();
    assert_eq!(marks_present(&doc).len(), 3);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Watermark).unwrap(), 1);
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Background"), "the background stays behind: {s:?}");
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::Background).unwrap(), 1);
    assert_eq!(streams(&reopen(&doc), 0), original, "back to exactly the original content");
}

#[test]
fn watermarks_rotate_fade_and_can_go_behind() {
    let mut doc = fixture();
    let wm = Watermark { text: "CONFIDENTIAL\nDo not copy".into(), opacity: 0.3, rotation: 45.0, behind: true, ..Watermark::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let s = streams(&doc, 0);
    assert!(s[0].contains("/PCMark /Watermark"), "behind: first");
    assert!(s[0].contains("/PCGS30 gs") && s[0].contains("0.707 0.707 -0.707 0.707 300 400 cm"), "{}", s[0]);
    assert!(s[0].contains("(CONFIDENTIAL) Tj") && s[0].contains("(Do not copy) Tj"));
    assert_eq!(s.len(), 2, "no wrapper needed for content behind");
}

#[test]
fn invalid_requests_change_nothing() {
    let mut doc = fixture();
    assert!(matches!(add_header_footer(&mut doc, &[0], &HeaderFooter::default(), false, &cx()), Err(EditError::Invalid(_))));
    assert!(matches!(add_watermark(&mut doc, &[0], &Watermark::default(), false), Err(EditError::Invalid(_))));
    assert_eq!(
        add_background(&mut doc, &[9], &Background { color: [1.0; 3], opacity: 1.0, ..Background::default() }, false),
        Err(EditError::NoSuchPage(9))
    );
    assert!(!doc.is_modified());
}

#[test]
fn flattening_draws_appearances_into_the_page_and_removes_the_comments() {
    use pdfcraft_annot::{Meta, NewAnnotation, NoteIcon, Shape, Style, add_annotation, add_reply};
    let mut doc = fixture();
    let meta = Meta { date: None, id: "x".into() };
    let add = |doc: &mut Document, shape: Shape| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: "c".into(), author: "a".into() }, &meta).unwrap()
    };
    add(&mut doc, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] });
    let note = add(&mut doc, Shape::Note { at: [200.0, 700.0], icon: NoteIcon::Comment });
    add_reply(&mut doc, 0, note, "reply", "b", &meta).unwrap();
    let before = streams(&doc, 0);
    let n = flatten(&mut doc, &[0], true, false).unwrap();
    assert_eq!(n, 2, "the rectangle and the note icon are drawn; the reply has nothing to draw");
    let doc = reopen(&doc);
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(!p.dict.contains(b"Annots"), "comments, pop-up and reply are gone");
    let s = streams(&doc, 0);
    assert_eq!(s.len(), before.len() + 3, "wrapped original + flattened content: {s:?}");
    let flat = s.last().unwrap();
    assert!(flat.contains("/PCFl0 Do") && flat.contains("/PCFl1 Do"), "{flat}");
    // The rectangle's appearance is drawn at its rectangle (bbox = rect, identity mapping).
    assert!(flat.contains("q 1 0 0 1 0 0 cm /PCFl0 Do Q"), "{flat}");
    // The note's 20×20 icon box is mapped onto its rect at (200, 680).
    assert!(flat.contains("1 0 0 1 200 680 cm /PCFl1 Do"), "{flat}");
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    assert!(xo.as_dict().unwrap().contains(b"PCFl1"));
    assert!(marks_present(&doc).is_empty(), "flattened content is not a removable mark");
}

#[test]
fn added_text_and_images_are_page_content_that_stays_editable() {
    let mut doc = fixture();
    let text = AddedText { rect: [72.0, 600.0, 300.0, 700.0], text: "Approved by Ada\nSecond line".into(), size: 14.0, ..AddedText::default() };
    assert_eq!(add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap(), 0);
    // An 1×1 gray image, placed on the rotated page.
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(1));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let img = doc.add(Object::Stream(Stream::from_raw(d, vec![128])));
    add_content(&mut doc, 1, &Content::Image(AddedImage::new([10.0, 10.0, 110.0, 60.0], img))).unwrap();
    let doc2 = reopen(&doc);
    let all = list_added(&doc2);
    assert_eq!(all.len(), 2);
    let Content::Text(t) = &all[0].content else { panic!() };
    assert_eq!((t.text.as_str(), t.size), ("Approved by Ada\nSecond line", 14.0));
    assert_eq!(t.rect, [72.0, 700.0 - 2.0 * 14.0 * 1.2, 300.0, 700.0], "the box height follows the two lines");
    let page0 = streams(&doc2, 0).join("\n");
    assert!(page0.contains("(Approved by Ada) Tj") && page0.contains("(Second line) Tj") && page0.contains("/PCFHelvetica 14 Tf"), "{page0}");
    // The rotated page draws in display space: the view matrix comes first.
    let page1 = streams(&doc2, 1).join("\n");
    assert!(page1.contains("q 0 1 -1 0 600 0 cm") && page1.contains(&format!("/PCImg{} Do", img.num)), "{page1}");
    // Edit: move, restyle, retype; then delete.
    let mut doc = doc2;
    let moved = AddedText {
        rect: [100.0, 500.0, 300.0, 520.0],
        text: "Approved".into(),
        bold: true,
        family: Family::Times,
        align: Align::Right,
        color: [1.0, 0.0, 0.0],
        ..text
    };
    update_content(&mut doc, 0, 0, &Content::Text(moved)).unwrap();
    let page0 = streams(&doc, 0).join("\n");
    assert!(page0.contains("/PCFTimesBold 14 Tf 1 0 0 rg") && !page0.contains("Second line"), "{page0}");
    assert!(
        update_content(&mut doc, 1, 0, &Content::Text(AddedText { text: "x".into(), rect: [0.0, 0.0, 50.0, 10.0], ..AddedText::default() })).is_err(),
        "kinds don't change"
    );
    delete_content(&mut doc, 0, 0).unwrap();
    assert_eq!(list_added(&doc).len(), 1);
    assert!(!streams(&doc, 0).join("").contains("Approved"));
    assert!(add_content(&mut doc, 0, &Content::Text(AddedText { rect: [0.0, 0.0, 100.0, 10.0], ..AddedText::default() })).is_err(), "empty text");
    // Page marks ignore added items.
    assert!(marks_present(&doc).is_empty());
}

/// One page with Helvetica (WinAnsi) and a subset font that has only the glyphs it uses.
fn text_page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        // Subset: glyphs for a (97) and b (98) only.
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Arial /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>"
            .into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn descriptor_text_page() -> Document {
    let content = "BT /F1 12 Tf 72 700 Td (ab) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+NeutralFace /FirstChar 97 /LastChar 98 /Widths [500 500] /FontDescriptor 6 0 R /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /FontDescriptor /FontName /NeutralFace /Flags 262208 /ItalicAngle -12 /FontWeight 700 /Ascent 900 /Descent -250 >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// The Japanese fallback face comes from craft-fonts, an optional build input.
fn without_craft_fonts(test: &str) -> bool {
    if pdfcraft_fonts::document_japanese_font().is_some() {
        return false;
    }
    eprintln!("skipping {test}: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
    true
}

#[test]
fn japanese_line_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_line_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "25362738こんにちは、お元気ですか？ 2.3444";
    let result = text::replace_line(&mut doc, 0, 0, replacement).unwrap();
    assert_eq!(result.substituted.as_deref(), Some("Shippori Mincho Type3"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCJp"), "{content}");
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, replacement);
}

#[test]
fn japanese_paragraph_uses_unicode_type3_fallback() {
    if without_craft_fonts("japanese_paragraph_uses_unicode_type3_fallback") {
        return;
    }
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let replacement = "こんにちは、お元気ですか？ 2.3444 日本語の文章";
    text::replace_block(&mut doc, 0, 0, replacement).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_blocks(&reopened, 0).unwrap()[0].text, replacement);
}

/// Without craft-fonts there is no Japanese face: editing in Japanese is a clear error that
/// leaves the page untouched, never a panic; Latin edits work as before.
#[test]
fn japanese_edit_without_craft_fonts_is_a_clear_error() {
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    let before = page_content_bytes(&doc, 0);
    let line = text::replace_line(&mut doc, 0, 0, "日本語の文字");
    let block = text::replace_block(&mut doc, 0, 0, "日本語の文字");
    if pdfcraft_fonts::document_japanese_font().is_some() {
        eprintln!("built with craft-fonts: the Japanese edits succeed (checked by the tests above)");
        assert!(line.is_ok() && block.is_ok());
        return;
    }
    for result in [line.map(|_| ()), block.map(|_| ())] {
        let Err(EditError::Invalid(msg)) = result else { panic!("expected a clear error, got {result:?}") };
        assert!(msg.contains("Japanese fallback font") && msg.contains("CRAFT_FONTS_DIR"), "{msg}");
    }
    assert_eq!(page_content_bytes(&doc, 0), before);
    let latin = text::replace_line(&mut doc, 0, 0, "Hello").unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "Hello", "{latin:?}");
}
#[test]
fn font_descriptor_style_is_exposed_even_with_a_neutral_name() {
    let doc = descriptor_text_page();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].bold && lines[0].italic, "{:?}", lines[0]);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert!(blocks[0].bold && blocks[0].italic, "{:?}", blocks[0]);
}

#[test]
fn mixed_font_runs_are_not_merged_into_one_source_style() {
    let doc = text_page("BT /F1 12 Tf 72 700 Td (Regular) Tj /F2 12 Tf 150 700 Td (Subset) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 2);
    assert_ne!(lines[0].font, lines[1].font);
}

#[test]
fn missing_glyphs_preserve_source_style_in_fallback() {
    let mut doc = descriptor_text_page();
    let result = text::replace_line(&mut doc, 0, 0, "Styled €").unwrap();
    assert_eq!(result.substituted.as_deref(), Some("Helvetica-BoldOblique"));
    let bytes = page_content_bytes(&doc, 0);
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("/PCEdHelveticaBoldOblique 12 Tf"), "{content}");
}

#[test]
fn text_lines_are_found_and_replaced_in_place() {
    let mut doc =
        text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 40 0 Td [(wor) -20 (ld)] TJ 0 -20 Td (Second line) Tj ET BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["Hello world", "Second line", "ab"]);
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01 && lines[0].rect[1] < 700.0 && lines[0].rect[3] > 700.0, "{:?}", lines[0].rect);
    assert!((lines[0].size - 12.0).abs() < 1e-9 && lines[0].base_font == "Helvetica");
    let second = lines[1].rect;
    let r = text::replace_line(&mut doc, 0, 0, "Goodbye, café").unwrap();
    assert_eq!(r.substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].text, "Goodbye, café");
    assert_eq!(lines[1].text, "Second line");
    assert_eq!(lines[1].rect, second);
}

/// A page whose content is one stream in three pieces, split between tokens: a marked-content
/// dictionary closes at the start of the middle piece, and the `TJ` array that ends it is shown
/// by the operator that starts the last one.
fn split_streams_page() -> Document {
    let piece = |s: &str| format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len());
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        piece("/P << /MCID 0"),
        piece(">> BDC BT /F1 12 Tf 72 700 Td (Target) Tj 0 -20 Td [(After) -20 (wards)]"),
        piece("TJ ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// The page's pieces joined back into one stream: every operator must still have its operands.
fn assert_split_tokens_kept(doc: &Document, new_text: &str) {
    let joined = streams(doc, 0).join("\n");
    assert!(joined.contains(new_text) && !joined.contains("Target"), "{joined}");
    assert!(joined.contains("[(After) -20 (wards)]"), "the text after the line is gone: {joined}");
    for op in pdfcraft_content::parse(joined.as_bytes()).ops {
        let want = match op.op.as_slice() {
            b"BDC" => 2,
            b"TJ" => 1,
            _ => continue,
        };
        assert_eq!(op.operands.len(), want, "{} lost its operands: {joined}", String::from_utf8_lossy(&op.op));
    }
}

#[test]
fn editing_text_keeps_the_tokens_a_stream_shares_with_its_neighbours() {
    let mut doc = split_streams_page();
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Target");
    text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
    assert_split_tokens_kept(&reopen(&doc), "Edited");
    // A paragraph rewrite rebuilds the same piece.
    let mut doc = split_streams_page();
    text::replace_block(&mut doc, 0, 0, "Rewrapped").unwrap();
    assert_split_tokens_kept(&reopen(&doc), "Rewrapped");
}

#[test]
fn missing_glyphs_substitute_helvetica_and_impossible_text_is_refused() {
    let mut doc = text_page("BT /F2 10 Tf 72 600 Td (ab) Tj ET");
    // "c" has no glyph in the subset: Helvetica takes over for this line.
    let r = text::replace_line(&mut doc, 0, 0, "abc").unwrap();
    assert_eq!(r.substituted.as_deref(), Some("Helvetica"));
    let doc2 = reopen(&doc);
    let lines = text::text_lines(&doc2, 0).unwrap();
    assert_eq!((lines[0].text.as_str(), lines[0].base_font.as_str()), ("abc", "Helvetica"));
    // Neither font can show Greek.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert!(text::replace_line(&mut doc, 0, 0, "Ωmega").is_err());
    assert!(text::replace_line(&mut doc, 0, 5, "x").is_err(), "no such line");
}

#[test]
fn paragraphs_are_found_and_rewrapped() {
    let mut doc = text_page(
        "BT 0 0 1 rg /F1 10 Tf 12 TL 72 700 Td (The quick brown fox) Tj T* (jumps over the) Tj T* (lazy dog.) Tj ET \
         BT /F1 10 Tf 72 600 Td (Next paragraph) Tj ET",
    );
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["The quick brown fox jumps over the lazy dog.", "Next paragraph"]);
    assert_eq!(blocks[0].lines, [0, 1, 2]);
    let width = blocks[0].rect[2] - blocks[0].rect[0];
    let next = blocks[1].rect;
    let long = "PdfCraft rewraps a paragraph to its own width when its text changes, keeping the font, size, colour and line spacing.";
    assert_eq!(text::replace_block(&mut doc, 0, 0, long).unwrap().substituted, None);
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, long);
    assert!(blocks[0].lines.len() > 3, "more lines: {}", blocks[0].lines.len());
    for i in &blocks[0].lines {
        assert!(lines[*i].rect[2] - lines[*i].rect[0] <= width + 1.0, "line {} fits", lines[*i].text);
    }
    // Same left edge, same spacing (12 pt).
    assert!((lines[0].rect[0] - 72.0).abs() < 0.01);
    let spacing = lines[blocks[0].lines[0]].origin_baseline() - lines[blocks[0].lines[1]].origin_baseline();
    assert!((spacing - 12.0).abs() < 0.01, "{spacing}");
    // The next paragraph is untouched, and the new text is blue like the old.
    assert_eq!(blocks[1].text, "Next paragraph");
    assert_eq!(blocks[1].rect, next);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains("0 0 1 rg"), "{content}");
    // Shorter text: fewer lines.
    let mut doc = doc;
    text::replace_block(&mut doc, 0, 0, "Short.").unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!((blocks[0].text.as_str(), blocks[0].lines.len()), ("Short.", 1));
}

fn page_content_bytes(doc: &Document, page: usize) -> Vec<u8> {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let c = p.dict.get(b"Contents").unwrap();
    match &*doc.resolve(c) {
        pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
        pdfcraft_cos::Object::Array(a) => a
            .iter()
            .flat_map(|x| match &*doc.resolve(x) {
                pdfcraft_cos::Object::Stream(s) => s.decoded().unwrap(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A page drawing image XObject /Im0 at 100,100 size 200 × 100 (pixels 4 × 2).
fn image_page() -> Document {
    let content = "q 200 0 0 100 100 100 cm /Im0 Do Q BT /F1 12 Tf 72 700 Td (Caption) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 8 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\u{0}\nendstream".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn page_images_move_turn_replace_and_delete() {
    let mut doc = image_page();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!(imgs.len(), 1);
    assert!(close(imgs[0].rect, [100.0, 100.0, 300.0, 200.0]), "{:?}", imgs[0].rect);
    assert_eq!((imgs[0].width, imgs[0].height, imgs[0].name.as_str()), (4, 2, "Im0"));
    // Move and resize.
    let t = images::rect_to_rect(imgs[0].rect, [50.0, 400.0, 150.0, 450.0]);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let doc = reopen(&doc);
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [50.0, 400.0, 150.0, 450.0]), "{r:?}");
    // A quarter turn about the centre: 100 × 50 becomes 50 × 100 around (100, 425).
    let mut doc = doc;
    let t = images::turn_about_centre(r, 1, false, false);
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Transform(t)).unwrap();
    let r = images::page_images(&doc, 0).unwrap()[0].rect;
    assert!(close(r, [75.0, 375.0, 125.0, 475.0]), "{r:?}");
    // The text after it is untouched.
    assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Caption");
    // Replace with another image object, in the same place.
    let mut d = pdfcraft_cos::Dict::new();
    for (k, v) in [
        (&b"Type"[..], pdfcraft_cos::Object::name("XObject")),
        (b"Subtype", pdfcraft_cos::Object::name("Image")),
        (b"Width", pdfcraft_cos::Object::Int(1)),
        (b"Height", pdfcraft_cos::Object::Int(1)),
    ] {
        d.set(k.to_vec(), v);
    }
    let other = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(d, vec![0])));
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Replace(other)).unwrap();
    let imgs = images::page_images(&doc, 0).unwrap();
    assert_eq!((imgs[0].object, imgs[0].width), (Some(other), 1));
    assert!(close(imgs[0].rect, r));
    // Delete.
    images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).unwrap();
    assert!(images::page_images(&doc, 0).unwrap().is_empty());
    assert!(images::change_image(&mut doc, 0, 0, &images::ImageChange::Delete).is_err());
}

#[test]
fn paragraphs_take_new_formatting() {
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    let style = text::BlockStyle {
        family: Some((added::Family::Times, true, false)),
        size: Some(12.0),
        color: Some([1.0, 0.0, 0.0]),
        align: Some(added::Align::Center),
        ..Default::default()
    };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(blocks[0].text, before.text, "same words");
    assert_eq!((blocks[0].base_font.as_str(), blocks[0].size), ("Times-Bold", 12.0));
    // Centred in the paragraph's width.
    let mid = (before.rect[0] + before.rect[2]) / 2.0;
    for i in &blocks[0].lines {
        let r = lines[*i].rect;
        assert!(((r[0] + r[2]) / 2.0 - mid).abs() < 2.0, "line {:?} centred on {mid}", lines[*i].text);
    }
    assert!(String::from_utf8_lossy(&page_content_bytes(&doc, 0)).contains("1 0 0 rg"));
    // Right alignment keeps lines flush with the right edge.
    let mut doc = text_page("BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET");
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Right), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
}

#[test]
fn a_new_paragraph_colour_does_not_spill_into_the_text_after_it() {
    let mut doc = text_page("BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj ET");
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let doc = reopen(&doc);
    // The fill colour in force where the second paragraph is shown (q/Q nest it).
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut fill, mut stack) = (String::from("0 g"), Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"q" => stack.push(fill.clone()),
            b"Q" => fill = stack.pop().unwrap_or_default(),
            b"g" | b"rg" | b"k" => fill = String::from_utf8_lossy(&pdfcraft_content::serialize_ops(std::slice::from_ref(op))).trim().to_string(),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Second paragraph") => break,
            _ => {}
        }
    }
    assert_eq!(fill, "0 g", "{ops:?}");
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[1].text, "Second paragraph");
}

#[test]
fn a_rewritten_paragraph_keeps_its_place_in_a_shared_text_object() {
    // Three paragraphs in one BT … ET: rewriting the second keeps the order (paragraph numbers
    // and reading order) and every paragraph's position.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let before = text::text_blocks(&doc, 0).unwrap();
    text::replace_block(&mut doc, 0, 1, "Edited second").unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = after.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["First paragraph", "Edited second", "Third paragraph"]);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[2].rect, before[2].rect);
    assert!((after[1].rect[0] - before[1].rect[0]).abs() < 0.01 && (after[1].rect[1] - before[1].rect[1]).abs() < 0.01, "{:?}", after[1].rect);
}

#[test]
fn recolouring_a_paragraph_in_a_shared_text_object_keeps_order_and_nesting() {
    // Splitting the text object (to keep the order) and q … Q (to keep the colour in) together:
    // q/Q stay outside text objects, and the paragraph after it keeps the original colour.
    let src = "BT /F1 10 Tf 72 700 Td (First paragraph) Tj 0 -40 Td (Second paragraph) Tj 0 -40 Td (Third paragraph) Tj ET";
    let mut doc = text_page(src);
    let style = text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 1, None, &style).unwrap();
    let doc = reopen(&doc);
    let texts: Vec<String> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["First paragraph", "Second paragraph", "Third paragraph"]);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let (mut in_text, mut depth, mut red) = (false, 0usize, Vec::new());
    for op in &ops {
        match op.op.as_slice() {
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"q" | b"Q" => {
                assert!(!in_text, "q/Q inside a text object: {ops:?}");
                depth = if op.is("q") { depth + 1 } else { depth.saturating_sub(1) };
            }
            b"rg" => red.push(depth),
            b"Tj" if op.operands.first().and_then(|o| o.as_string()).is_some_and(|s| s.to_text() == "Third paragraph") => {
                assert!(red.iter().all(|d| *d > depth), "red still in force for the third paragraph: {ops:?}");
            }
            _ => {}
        }
    }
    assert!(!red.is_empty(), "{ops:?}");
}

#[test]
fn justify_underline_and_spacing() {
    let src = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj T* (end) Tj ET";
    // Justified: every line but the last reaches the right edge.
    let mut doc = text_page(src);
    let right = text::text_blocks(&doc, 0).unwrap()[0].rect[2];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { align: Some(added::Align::Justify), ..Default::default() }).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    for l in &lines[..lines.len() - 1] {
        assert!((l.rect[2] - right).abs() < 1.0, "{:?} ends at {} not {right}", l.text, l.rect[2]);
    }
    // Double line spacing, underlined.
    let mut doc = text_page(src);
    let style = text::BlockStyle { line_spacing: Some(2.0), underline: Some(true), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let lines = text::text_lines(&doc, 0).unwrap();
    assert!((lines[0].origin_baseline() - lines[1].origin_baseline() - 20.0).abs() < 0.01);
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert!(content.contains(" l\nS\n") || content.contains(" l S"), "{content}");
    // Character spacing widens, horizontal scale narrows.
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    let w0 = text::text_lines(&doc, 0).unwrap()[0].rect;
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { char_spacing: Some(2.0), ..Default::default() }).unwrap();
    let w1 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!((w1[2] - w1[0]) - (w0[2] - w0[0]) > 15.0, "{w0:?} → {w1:?}");
    let mut doc = text_page("BT /F1 10 Tf 100 700 Td (Wide text) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { scale: Some(50.0), ..Default::default() }).unwrap();
    let w2 = text::text_lines(&doc, 0).unwrap()[0].rect;
    assert!(((w2[2] - w2[0]) - (w0[2] - w0[0]) / 2.0).abs() < 1.0, "{w0:?} → {w2:?}");
}

#[test]
fn a_line_drawn_twice_is_replaced_everywhere() {
    // Fake bold: a line drawn twice at the same spot (each copy its own text object). The two
    // copies group apart, but editing one replaces both — a surviving copy would show the old
    // text under the new.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "the copies group apart");
    text::replace_block(&mut doc, 0, 0, "Hello world").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello world"], "one line, replaced");
    let content = String::from_utf8_lossy(&page_content_bytes(&doc, 0)).into_owned();
    assert_eq!(content.matches("(Hello").count(), 1, "the second copy's operators are gone: {content}");
    // Copies in one text object (the matrix re-issued at the same spot) go too.
    let mut doc = text_page("BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hi) Tj 1 0 0 1 72.3 700 Tm (Hi) Tj ET");
    text::replace_line(&mut doc, 0, 0, "Hey").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hey"], "one line, replaced");
    // A neighbouring line is not coincident and stays (40 pt spacing groups apart at 12 pt).
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj 0 -40 Td (world) Tj ET");
    text::replace_block(&mut doc, 0, 0, "Hello there").unwrap();
    let doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello there", "world"]);
}

#[test]
fn paragraphs_move_and_rewrap_to_a_new_width() {
    let para = "BT /F1 10 Tf 12 TL 100 700 Td (One two three four five six seven) Tj T* (eight nine ten eleven twelve) Tj ET";
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Moved 50 right and 200 down, same words and wrapping.
    let mut doc = text_page(para);
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([50.0, -200.0]), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    let after = text::text_blocks(&doc, 0).unwrap()[0].clone();
    assert_eq!((after.text.as_str(), after.lines.len()), (before.text.as_str(), before.lines.len()));
    assert!(close(after.rect[0], before.rect[0] + 50.0) && close(after.rect[1], before.rect[1] - 200.0), "{:?} → {:?}", before.rect, after.rect);
    // Drawn at half scale: the move is still in page space.
    let mut doc = text_page(&format!("q 0.5 0 0 0.5 0 0 cm {} Q", para.replace("100 700 Td", "200 1400 Td")));
    let before = text::text_blocks(&doc, 0).unwrap()[0].clone();
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { offset: Some([30.0, 40.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(close(after.rect[0], before.rect[0] + 30.0) && close(after.rect[1], before.rect[1] + 40.0), "{:?} → {:?}", before.rect, after.rect);
    // A narrower box rewraps into more lines that fit it; a wider one into fewer.
    let mut doc = text_page(para);
    let left = text::text_blocks(&doc, 0).unwrap()[0].rect[0];
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(80.0), ..Default::default() }).unwrap();
    let narrow = text::text_blocks(&reopen(&doc), 0).unwrap()[0].clone();
    assert!(narrow.lines.len() > 2, "{narrow:?}");
    assert!(narrow.rect[2] <= left + 80.5, "{narrow:?}");
    let mut doc = text_page(para);
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(400.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].lines.len(), 1);
    // Untrusted numbers (the automation tools pass them through) are refused, not drawn.
    let mut doc = text_page(para);
    for style in [
        text::BlockStyle { offset: Some([f64::NAN, 0.0]), ..Default::default() },
        text::BlockStyle { offset: Some([0.0, f64::INFINITY]), ..Default::default() },
        text::BlockStyle { width: Some(f64::NAN), ..Default::default() },
    ] {
        assert!(text::rewrite_block(&mut doc, 0, 0, None, &style).is_err(), "{style:?}");
    }
    // A zero width is clamped to one character's width rather than looping or vanishing.
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { width: Some(0.0), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap().iter().map(|b| b.text.split_whitespace().count()).sum::<usize>(), 12);
}

/// A page split into several content streams, as AutoCAD writes them: the first scales the
/// page (`0.12 0 0 0.12 0 0 cm`, no `q`), the text comes in a later one.
fn split_page(first: &str, second: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{first}\nendstream", first.len()),
        format!("<< /Length {} >>\nstream\n{second}\nendstream", second.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

#[test]
fn the_graphics_state_carries_over_between_content_streams() {
    let close = |a: f64, b: f64| (a - b).abs() < 0.5;
    // Text at (1000, 2000) in a space scaled by 0.12: on the page at (120, 240), 12 pt.
    let mut doc = split_page("0.12 0 0 0.12 0 0 cm", "BT /F1 100 Tf 1000 2000 Td (Site plan) Tj ET");
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let l = &lines[0];
    assert!(close(l.rect[0], 120.0) && l.rect[1] > 230.0 && l.rect[3] < 260.0, "{:?}", l.rect);
    assert!(close(l.size, 12.0), "{}", l.size);
    // Editing keeps the paragraph where it was, at its size; moving it moves it in page space.
    text::rewrite_block(&mut doc, 0, 0, Some("Site plan, rev. B"), &text::BlockStyle { offset: Some([10.0, -20.0]), ..Default::default() }).unwrap();
    let after = text::text_blocks(&reopen(&doc), 0).unwrap();
    assert_eq!(after[0].text, "Site plan, rev. B");
    assert!(close(after[0].rect[0], 130.0) && close(after[0].rect[1], l.rect[1] - 20.0), "{:?}", after[0].rect);
    assert!(close(after[0].size, 12.0), "{}", after[0].size);
    // A `q` left open in one stream and closed in the next still restores the state.
    let doc = split_page("q 0.5 0 0 0.5 0 0 cm", "Q BT /F1 10 Tf 100 100 Td (Footpath) Tj ET");
    let l = &text::text_lines(&doc, 0).unwrap()[0];
    assert!(close(l.rect[0], 100.0) && close(l.size, 10.0), "{l:?}");
}

fn set_content_pieces(doc: &mut Document, pieces: &[&str]) -> Vec<pdfcraft_cos::Object> {
    use pdfcraft_cos::{Dict, Object, Stream};
    let refs: Vec<_> = pieces.iter().map(|s| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), s.as_bytes().to_vec()))))).collect();
    let page = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(refs.clone()))).unwrap();
    refs
}

fn content_refs(doc: &Document) -> Vec<pdfcraft_cos::Object> {
    let page = &pdfcraft_model::pages(doc)[0];
    match &*doc.resolve(page.dict.get(b"Contents").unwrap()) {
        pdfcraft_cos::Object::Array(a) => a.clone(),
        _ => vec![page.dict.get(b"Contents").unwrap().clone()],
    }
}

#[test]
fn split_text_operators_are_selectable_and_replaceable() {
    let mut doc = split_streams_page();
    let before = text::text_lines(&doc, 0).unwrap();
    assert_eq!(before.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Afterwards"]);
    assert_eq!(before[1].origin_baseline(), 680.0);
    let refs = content_refs(&doc);
    let original = streams(&doc, 0);
    text::replace_line(&mut doc, 0, 1, "Edited").unwrap();
    assert_eq!(content_refs(&doc)[0], refs[0], "untouched piece keeps its reference");
    for (r, bytes) in refs.iter().zip(&original) {
        let object = doc.resolve(r);
        let pdfcraft_cos::Object::Stream(stream) = &*object else { panic!("stream expected") };
        assert_eq!(stream.decoded().unwrap(), bytes.as_bytes(), "shared originals are not modified");
    }
    let doc = reopen(&doc);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(after.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Target", "Edited"]);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[1].origin_baseline(), before[1].origin_baseline());
    let joined = streams(&doc, 0).join("\n");
    let parsed = pdfcraft_content::parse(joined.as_bytes());
    assert_eq!(parsed.skipped, 0);
    assert_eq!(parsed.ops.iter().find(|op| op.is("BDC")).unwrap().operands.len(), 2);
    assert!(!joined.contains("(After)"));
}

#[test]
fn a_split_text_operator_can_be_restyled_in_its_original_text_object() {
    let mut doc = split_streams_page();
    let before = text::text_lines(&doc, 0).unwrap();
    let blocks = text::text_blocks(&doc, 0).unwrap();
    let index = blocks.iter().position(|b| b.text == "Afterwards").unwrap();
    text::rewrite_block(&mut doc, 0, index, None, &text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() }).unwrap();
    let doc = reopen(&doc);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].text, "Target");
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[0].color, before[0].color);
    assert_eq!(after[1].text, "Afterwards");
    assert_eq!(after[1].color, [1.0, 0.0, 0.0]);
    assert_eq!(after[1].origin_baseline(), 680.0);
    let joined = streams(&doc, 0).join("\n");
    let parsed = pdfcraft_content::parse(joined.as_bytes());
    assert_eq!(parsed.skipped, 0);
    let mut depth = 0;
    for op in parsed.ops {
        if op.is("BT") {
            depth += 1;
            assert_eq!(depth, 1);
        }
        if op.is("ET") {
            depth -= 1;
            assert_eq!(depth, 0);
        }
    }
    assert_eq!(depth, 0);
}

#[test]
fn text_matrices_and_a_single_line_continue_across_pieces() {
    let mut doc = split_page("BT /F1 12 Tf 72 700 Td (Hello) Tj", "( world) Tj 0 -20 Td (Next) Tj ET");
    let before = text::text_lines(&doc, 0).unwrap();
    assert_eq!(before.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello world", "Next"]);
    assert_eq!(before[1].origin_baseline(), 680.0);
    text::replace_line(&mut doc, 0, 0, "Changed").unwrap();
    let doc = reopen(&doc);
    let after = text::text_lines(&doc, 0).unwrap();
    assert_eq!(after.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Changed", "Next"]);
    assert_eq!(after[1].rect, before[1].rect);
}

#[test]
fn editing_split_image_operators_preserves_other_split_syntax_and_raw_bytes() {
    for operation in 0..3 {
        let mut doc = image_page();
        let pieces = [
            "/P << /MCID 0",
            ">> BDC % original spelling\n2.0000 Custom q 200 0 0 100 100 100 cm /Im0",
            "Do Q BT /F1 12 Tf 72 700 Td [(Cap)",
            "(tion)] TJ ET EMC % trailing comment",
        ];
        let refs = set_content_pieces(&mut doc, &pieces);
        let image = images::page_images(&doc, 0).unwrap()[0].clone();
        assert!(close(image.rect, [100.0, 100.0, 300.0, 200.0]));
        let caption = text::text_lines(&doc, 0).unwrap()[0].clone();
        let change = match operation {
            0 => images::ImageChange::Transform([1.0, 0.0, 0.0, 1.0, 10.0, 20.0]),
            1 => images::ImageChange::Replace(image.object.unwrap()),
            _ => images::ImageChange::Delete,
        };
        images::change_image(&mut doc, 0, 0, &change).unwrap();
        let after_refs = content_refs(&doc);
        assert_eq!(after_refs[0], refs[0]);
        assert_eq!(after_refs[3], refs[3]);
        let doc = reopen(&doc);
        let after = text::text_lines(&doc, 0).unwrap();
        assert_eq!(after[0].text, "Caption");
        assert_eq!(after[0].rect, caption.rect);
        let data = streams(&doc, 0);
        assert_eq!(data[0], pieces[0]);
        assert_eq!(data[3], pieces[3]);
        assert!(data[1].starts_with(">> BDC % original spelling\n2.0000 Custom q 200 0 0 100 100 100 cm "));
        assert!(data[2].ends_with(" Q BT /F1 12 Tf 72 700 Td [(Cap)"));
        let images = images::page_images(&doc, 0).unwrap();
        match operation {
            0 => assert!(close(images[0].rect, [110.0, 120.0, 310.0, 220.0])),
            1 => assert!(close(images[0].rect, image.rect)),
            _ => assert!(images.is_empty()),
        }
        let joined = data.join("\n");
        let parsed = pdfcraft_content::parse(joined.as_bytes());
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.ops.iter().find(|op| op.is("BDC")).unwrap().operands.len(), 2);
        assert_eq!(parsed.ops.iter().find(|op| op.is("TJ")).unwrap().operands.len(), 1);
    }
}

#[test]
fn replacing_text_preserves_unrelated_overlapping_captions_and_scopes() {
    for paragraph in [false, true] {
        for scoped in [false, true] {
            let mut doc = text_page("");
            let first = if scoped { "/English BMC BT /F1 12 Tf 72 700 Td (Caption) Tj ET EMC" } else { "BT /F1 12 Tf 72 700 Td (Caption) Tj ET" };
            let second = if scoped {
                // The same text in another marked-content scope is not a duplicate either.
                "/French BMC BT /F1 12 Tf 72 700 Td (Caption) Tj ET EMC"
            } else {
                "BT /F1 12 Tf 72 700 Td (Legende) Tj ET"
            };
            let refs = set_content_pieces(&mut doc, &[first, second]);
            if paragraph {
                text::replace_block(&mut doc, 0, 0, "Edited").unwrap();
            } else {
                text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
            }
            assert_eq!(content_refs(&doc)[1], refs[1], "unselected caption's object is retained");
            let doc = reopen(&doc);
            let after = text::text_lines(&doc, 0).unwrap();
            assert_eq!(after.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Edited", if scoped { "Caption" } else { "Legende" }]);
            assert_eq!(streams(&doc, 0)[1], second);
        }
    }
}

#[test]
fn paragraph_relocation_cannot_escape_marked_content_inside_bt() {
    // Includes an OCG property name and an MCID. No scope is allowed to disappear or become
    // visible outside its enclosing sequence, even when its BT is in an earlier piece.
    let mut doc = split_page("BT /F1 12 Tf 72 700 Td /OC /Hidden BDC /Span << /MCID 7 >> BDC", "(Secret) Tj EMC EMC ET");
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut layer = Dict::new();
    layer.set(b"Type".to_vec(), Object::name("OCG"));
    layer.set(b"Name".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Hidden".to_vec())));
    let layer_ref = doc.add(Object::Dict(layer));
    let mut properties = Dict::new();
    properties.set(b"Hidden".to_vec(), Object::Ref(layer_ref));
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    resources.set(b"Properties".to_vec(), Object::Dict(properties));
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let mut config = Dict::new();
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"OFF".to_vec(), Object::Array(vec![Object::Ref(layer_ref)]));
    let mut oc_properties = Dict::new();
    oc_properties.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(layer_ref)]));
    oc_properties.set(b"D".to_vec(), Object::Dict(config));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"OCProperties".to_vec(), Object::Dict(oc_properties.clone()))).unwrap();
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let error =
        text::rewrite_block(&mut doc, 0, 0, Some("Edited"), &text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() }).unwrap_err();
    assert!(error.to_string().contains("marked content"));
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before, "refusal changes no objects");
    // In-place line replacement preserves both scopes without relocating BT/ET.
    text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
    assert_eq!(doc.get(root).as_dict().unwrap().get(b"OCProperties"), Some(&Object::Dict(oc_properties)));
    let joined = streams(&reopen(&doc), 0).join("\n");
    let mut scope = Vec::new();
    for op in pdfcraft_content::parse(joined.as_bytes()).ops {
        if op.is("BDC") {
            scope.push(op.operands.clone());
        }
        if op.is("Tj") {
            assert_eq!(scope.len(), 2);
            assert_eq!(scope[0][0].as_name(), Some(&b"OC"[..]));
            assert_eq!(scope[0][1].as_name(), Some(&b"Hidden"[..]));
            assert_eq!(scope[1][1].as_dict().unwrap().int(b"MCID"), Some(7));
        }
        if op.is("EMC") {
            scope.pop();
        }
    }
    assert!(scope.is_empty());
}

#[test]
fn invalid_paragraph_font_sizes_return_errors_without_mutation() {
    for source_size in [12.0, 15_000.0] {
        for requested in [None, Some(15_000.0), Some(f64::INFINITY), Some(f64::NAN)] {
            if source_size == 12.0 && requested.is_none() {
                continue;
            }
            let mut doc = text_page(&format!("BT /F1 {source_size} Tf 72 700 Td (Text) Tj ET"));
            let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
            let style = text::BlockStyle { size: requested, width: Some(100.0), ..Default::default() };
            assert!(text::rewrite_block(&mut doc, 0, 0, Some("Changed"), &style).is_err());
            assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
}

#[test]
fn paragraph_edits_preserve_ideographic_spaces_without_fallback_fonts() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td ( A B ) Tj ET");
    // Contributor-original encoding data maps the existing space glyph to U+3000. This tests
    // normalization/reflow independently of any optional Japanese font or visual asset.
    let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange 3 beginbfchar <20> <3000> <41> <0041> <42> <0042> endbfchar";
    let cmap_ref = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), cmap.to_vec())));
    let page = &pdfcraft_model::pages(&doc)[0];
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
    let font = fonts.as_dict().unwrap().get(b"F1").unwrap().as_ref().unwrap();
    doc.update_dict(font, |d| d.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref))).unwrap();
    assert_eq!(text::text_blocks(&doc, 0).unwrap()[0].text, "　A　B　");
    text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() }).unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "　A　B　");
    text::replace_block(&mut doc, 0, 0, "B　　A").unwrap();
    assert_eq!(text::text_blocks(&reopen(&doc), 0).unwrap()[0].text, "B　　A");
}

fn set_content_stream(doc: &mut Document, stream: pdfcraft_cos::Stream) {
    let content = doc.add(Object::Stream(stream));
    let page = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
}

#[test]
fn editing_resolves_indirect_filter_and_predictor_metadata() {
    for array in [false, true] {
        let body = b"BT /F1 12 Tf 72 700 Td (Indirect) Tj ET";
        // A single TIFF-predictor row. Resolving Predictor=2 is essential: silently using
        // the default predictor does not recover a valid content stream.
        let predicted: Vec<_> = body.iter().enumerate().map(|(i, byte)| byte.wrapping_sub(if i == 0 { 0 } else { body[i - 1] })).collect();
        let mut doc = text_page("");
        let name = doc.add(Object::name("FlateDecode"));
        let predictor = doc.add(Object::Int(2));
        let default_colors = doc.add(Object::Null);
        let columns = doc.add(Object::Int(body.len() as i64));
        let mut params = Dict::new();
        params.set(b"Predictor".to_vec(), Object::Ref(predictor));
        params.set(b"Colors".to_vec(), Object::Ref(default_colors));
        params.set(b"Columns".to_vec(), Object::Ref(columns));
        let params = doc.add(Object::Dict(params));
        let (filter, params) =
            if array { (doc.add(Object::Array(vec![Object::Ref(name)])), doc.add(Object::Array(vec![Object::Ref(params)]))) } else { (name, params) };
        let mut stream = pdfcraft_cos::Stream::flate(Dict::new(), &predicted);
        stream.dict.set(b"F".to_vec(), Object::Null);
        stream.dict.set(b"Filter".to_vec(), Object::Ref(filter));
        stream.dict.set(b"DecodeParms".to_vec(), Object::Ref(params));
        set_content_stream(&mut doc, stream);
        assert_eq!(text::text_lines(&doc, 0).unwrap()[0].text, "Indirect");
        text::replace_line(&mut doc, 0, 0, "Edited").unwrap();
        assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].text, "Edited");
    }
}

#[test]
fn malformed_decode_metadata_is_refused_without_mutation() {
    for case in 0..9 {
        let mut doc = text_page("");
        let body = b"BT /F1 12 Tf 72 700 Td (Original) Tj ET";
        let mut stream =
            if case < 4 { pdfcraft_cos::Stream::from_raw(Dict::new(), body.to_vec()) } else { pdfcraft_cos::Stream::flate(Dict::new(), body) };
        match case {
            0 => stream.dict.set(b"Filter".to_vec(), Object::Int(7)),
            1 => stream.dict.set(b"Filter".to_vec(), Object::Array(vec![Object::Int(7)])),
            2 => stream.dict.set(b"Filter".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(999, 0))),
            3 => {
                let cycle = doc.add(Object::Null);
                doc.set(cycle, Object::Ref(cycle));
                stream.dict.set(b"Filter".to_vec(), Object::Ref(cycle));
            }
            4 => stream.dict.set(b"DecodeParms".to_vec(), Object::Int(7)),
            5 => stream.dict.set(b"DecodeParms".to_vec(), Object::Array(vec![])),
            6 | 7 => {
                let mut params = Dict::new();
                params.set(b"Predictor".to_vec(), if case == 6 { Object::name("invalid") } else { Object::Int(0) });
                stream.dict.set(b"DecodeParms".to_vec(), Object::Dict(params));
            }
            _ => stream.dict.set(b"F".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"external-content".to_vec()))),
        }
        set_content_stream(&mut doc, stream);
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(text::text_lines(&doc, 0).is_err(), "case {case} silently interpreted malformed metadata");
        assert!(text::replace_line(&mut doc, 0, 0, "Changed").is_err(), "case {case}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before, "case {case} changed objects");
    }
}

#[test]
fn unsupported_generated_text_preserves_existing_items_and_marks() {
    use crate::added::{AddedText, Content, add_content, list_added, update_content};
    let mut doc = fixture();
    let item = AddedText { text: "Keep this item".into(), rect: [50.0, 500.0, 500.0, 550.0], ..Default::default() };
    add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();
    let mut hf = HeaderFooter::default();
    hf.text[0] = "Keep this header".into();
    add_header_footer(&mut doc, &[0, 1], &hf, false, &cx()).unwrap();
    let mut wm = Watermark { text: "Keep this watermark".into(), ..Default::default() };
    add_watermark(&mut doc, &[0, 1], &wm, false).unwrap();
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let old_items = list_added(&doc);
    let unsupported = Content::Text(AddedText { text: "ASCII \u{0627}".into(), ..item });
    assert!(add_content(&mut doc, 0, &unsupported).unwrap_err().to_string().contains("U+0627"));
    assert!(update_content(&mut doc, 0, 0, &unsupported).is_err());
    hf.text[2] = "ASCII \u{0627}".into();
    assert!(add_header_footer(&mut doc, &[0, 1], &hf, true, &cx()).is_err());
    wm.text = "ASCII \u{0627}".into();
    assert!(add_watermark(&mut doc, &[0, 1], &wm, true).is_err());
    assert_eq!(list_added(&doc), old_items);
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn generated_cjk_roundtrips_and_keeps_separate_item_encodings() {
    use crate::added::{AddedText, Content, add_content, list_added};
    let mut doc = fixture();
    let make = |text: &str, top| Content::Text(AddedText { text: text.into(), rect: [50.0, top - 30.0, 500.0, top], ..Default::default() });
    let text = "ASCII \u{65e5}\u{672c}\u{8a9e} 123";
    if pdfcraft_fonts::document_japanese_font().is_none() {
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let error = add_content(&mut doc, 0, &make(text, 550.0)).unwrap_err().to_string();
        assert!(error.contains("CRAFT_FONTS_DIR"), "{error}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        return;
    }
    add_content(&mut doc, 0, &make(text, 550.0)).unwrap();
    // Different code 1 in a second Type 3 resource must not alter the first item.
    add_content(&mut doc, 0, &make("B \u{672c} Z", 450.0)).unwrap();
    let mut hf = HeaderFooter::default();
    hf.text[0] = "Header \u{65e5} <<1>>".into();
    add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
    let wm = Watermark { text: "Mark \u{8a9e}".into(), rotation: 0.0, ..Default::default() };
    add_watermark(&mut doc, &[0], &wm, false).unwrap();
    let doc = reopen(&doc);
    assert!(list_added(&doc).iter().any(|a| matches!(&a.content, Content::Text(t) if t.text == text)));
    // Decode the emitted bytes using each actual font resource, independent of /PCAdded.
    let p = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let mut shown = String::new();
    let page_contents = doc.resolve(p.dict.get(b"Contents").unwrap());
    let stream_list = match &*page_contents {
        Object::Array(items) => items.clone(),
        _ => vec![p.dict.get(b"Contents").unwrap().clone()],
    };
    for content in stream_list {
        let mut metrics = pdfcraft_fonts::pdf::Metrics::fallback();
        for op in pdfcraft_content::parse(&stream_bytes(&doc, &content).unwrap()).ops {
            if op.is("Tf") {
                let font = doc.resolve(fonts.as_dict().unwrap().get(op.name(0).unwrap()).unwrap());
                metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font.as_dict().unwrap());
            } else if op.is("Tj") {
                shown.push_str(&metrics.decode(&op.operands[0].as_string().unwrap().bytes));
            }
        }
    }
    for expected in [text, "B \u{672c} Z", "Header \u{65e5} 1", "Mark \u{8a9e}"] {
        assert!(shown.contains(expected), "{shown}");
    }
}

#[test]
fn cjk_styles_are_not_silently_replaced_with_regular() {
    use crate::added::{AddedText, Content, add_content};
    let mut doc = fixture();
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    for (bold, italic) in [(true, false), (false, true)] {
        let item = Content::Text(AddedText { text: "A \u{65e5}".into(), bold, italic, rect: [50.0, 500.0, 500.0, 550.0], ..Default::default() });
        assert!(add_content(&mut doc, 0, &item).unwrap_err().to_string().contains("regular"));
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn damaged_text_operand_spans_are_rejected_without_mutation() {
    for split in [false, true] {
        let mut doc = text_page("");
        let pieces: Vec<&str> =
            if split { vec!["BT /F1 12 Tf 72 700 Td (Original)", "] Tj ET"] } else { vec!["BT /F1 12 Tf 72 700 Td (Original) ] Tj ET"] };
        let refs = set_content_pieces(&mut doc, &pieces);
        assert!(pdfcraft_content::parse(pieces.join("\n").as_bytes()).skipped > 0);
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(text::text_lines(&doc, 0).unwrap_err().to_string().contains("damaged content syntax"));
        assert!(text::replace_line(&mut doc, 0, 0, "Edited").unwrap_err().to_string().contains("damaged content syntax"));
        assert!(text::replace_block(&mut doc, 0, 0, "Edited").unwrap_err().to_string().contains("damaged content syntax"));
        assert_eq!(content_refs(&doc), refs);
        assert_eq!(streams(&doc, 0), pieces);
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn damaged_image_operand_spans_are_rejected_without_mutation() {
    for split in [false, true] {
        let mut doc = image_page();
        let image = images::page_images(&doc, 0).unwrap()[0].object.unwrap();
        let pieces: Vec<&str> = if split { vec!["q 200 0 0 100 100 100 cm /Im0", "] Do Q"] } else { vec!["q 200 0 0 100 100 100 cm /Im0 ] Do Q"] };
        let refs = set_content_pieces(&mut doc, &pieces);
        assert!(pdfcraft_content::parse(pieces.join("\n").as_bytes()).skipped > 0);
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(images::page_images(&doc, 0).unwrap_err().to_string().contains("damaged content syntax"));
        for change in
            [images::ImageChange::Delete, images::ImageChange::Replace(image), images::ImageChange::Transform([1.0, 0.0, 0.0, 1.0, 10.0, 20.0])]
        {
            assert!(images::change_image(&mut doc, 0, 0, &change).unwrap_err().to_string().contains("damaged content syntax"));
            assert_eq!(content_refs(&doc), refs);
            assert_eq!(streams(&doc, 0), pieces);
            assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
}

#[test]
fn failed_paragraph_reflow_rolls_back_allocated_type3_font() {
    if without_craft_fonts("failed_paragraph_reflow_rolls_back_allocated_type3_font") {
        return;
    }
    let source = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
    let replacement = "\u{65e5}\u{672c}\u{8a9e}";
    // Prove this fixture needs, and can allocate, the fallback before testing later errors.
    let mut control = source.clone();
    let result = text::rewrite_block(&mut control, 0, 0, Some(replacement), &Default::default()).unwrap();
    assert!(result.substituted.as_deref().is_some_and(|name| name.contains("Type3")));
    assert!(control.object_numbers().len() > source.object_numbers().len());
    for (style, message) in [
        (text::BlockStyle { width: Some(f64::NAN), ..Default::default() }, "width"),
        (text::BlockStyle { width: Some(f64::INFINITY), ..Default::default() }, "width"),
        (text::BlockStyle { offset: Some([f64::INFINITY, 0.0]), ..Default::default() }, "moved"),
        (text::BlockStyle { offset: Some([0.0, f64::NAN]), ..Default::default() }, "moved"),
    ] {
        let mut doc = source.clone();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let error = text::rewrite_block(&mut doc, 0, 0, Some(replacement), &style).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
        assert_eq!(doc.object_numbers(), source.object_numbers());
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

fn install_legacy_font_content(doc: &mut Document, legacy: pdfcraft_cos::ObjRef, names: &[String]) -> Object {
    let page = pdfcraft_model::pages(doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    let imported = pdfcraft_fonts::generated::standard_font("Courier-Bold");
    for name in names {
        fonts.set(name.as_bytes().to_vec(), imported.clone());
    }
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let content = format!("BT /{} 12 Tf 1 0 0 1 50 300 Tm (Legacy text) Tj ET", names[0]);
    doc.set(legacy, Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), content.into_bytes())));
    let mut list = match page.dict.get(b"Contents") {
        Some(content) => match &*doc.resolve(content) {
            Object::Array(items) => items.clone(),
            _ => vec![content.clone()],
        },
        None => Vec::new(),
    };
    list.push(Object::Ref(legacy));
    doc.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Array(list));
    })
    .unwrap();
    imported
}

#[test]
fn generated_latin_fonts_keep_imported_primary_resources() {
    use crate::added::{AddedText, Content, add_content};
    let mut doc = fixture();
    let legacy = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), vec![])));
    let names = ["PCFHelvetica".to_string(), "PCHelv".to_string()];
    let imported = install_legacy_font_content(&mut doc, legacy, &names);
    let old_content = doc.get(legacy);
    add_content(&mut doc, 0, &Content::Text(AddedText { text: "Added".into(), rect: [50.0, 500.0, 500.0, 550.0], ..Default::default() })).unwrap();
    let mut hf = HeaderFooter::default();
    hf.text[0] = "Header".into();
    // Duplicate page selections must also reserve names installed by the first iteration.
    add_header_footer(&mut doc, &[0, 0], &hf, false, &cx()).unwrap();
    add_watermark(&mut doc, &[0], &Watermark { text: "Watermark".into(), ..Default::default() }, false).unwrap();
    let fonts = page_fonts(&doc, &pdfcraft_model::pages(&doc)[0]);
    for name in names {
        assert_eq!(fonts.get(name.as_bytes()), Some(&imported));
    }
    assert_eq!(doc.get(legacy), old_content);
    assert!(fonts.contains(b"PCFHelvetica_") && fonts.contains(b"PCHelv_"));
}

#[test]
fn generated_cjk_fonts_reserve_imported_names_even_when_object_numbers_match() {
    use crate::added::{AddedText, Content, add_content};
    use pdfcraft_fonts::generated::{TextFonts, standard_font};
    if pdfcraft_fonts::document_japanese_font().is_none() {
        return;
    }
    for kind in 0..3 {
        let mut doc = fixture();
        let legacy = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), vec![])));
        // Probe only a clone to find the new object's name, then simulate an imported PDF
        // that already uses that unrelated name (and its first alternate) for Courier.
        let mut probe = doc.clone();
        let prepared = TextFonts::new(&mut probe, "Latin", standard_font("Helvetica"), "\u{65e5}").unwrap();
        let collision = prepared.fonts.iter().find(|(name, _)| name.starts_with(b"PCUnicode")).unwrap().0.clone();
        let name = String::from_utf8(collision).unwrap();
        let names = [name.clone(), format!("{name}_")];
        let imported = install_legacy_font_content(&mut doc, legacy, &names);
        let old_content = doc.get(legacy);
        match kind {
            0 => {
                add_content(
                    &mut doc,
                    0,
                    &Content::Text(AddedText { text: "\u{65e5}".into(), rect: [50.0, 500.0, 500.0, 550.0], ..Default::default() }),
                )
                .unwrap();
            }
            1 => {
                let mut hf = HeaderFooter::default();
                hf.text[0] = "\u{65e5}".into();
                add_header_footer(&mut doc, &[0], &hf, false, &cx()).unwrap();
            }
            _ => {
                add_watermark(&mut doc, &[0], &Watermark { text: "\u{65e5}".into(), ..Default::default() }, false).unwrap();
            }
        }
        let fonts = page_fonts(&doc, &pdfcraft_model::pages(&doc)[0]);
        for reserved in &names {
            assert_eq!(fonts.get(reserved.as_bytes()), Some(&imported));
        }
        assert_eq!(doc.get(legacy), old_content);
        let actual = doc.resolve(fonts.get(format!("{name}__").as_bytes()).expect("a fresh destination name"));
        let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, actual.as_dict().unwrap());
        assert_eq!(metrics.decode(&[1]), "\u{65e5}");
    }
}

#[test]
fn paragraph_reflow_preserves_quote_moves_and_spacing_for_untouched_text() {
    for split_text_object in [false, true] {
        for separate_pieces in [false, true] {
            for double_quote in [false, true] {
                let earlier = if split_text_object { "(Earlier) Tj 0 0 1 rg" } else { "" };
                let (first, second, quote) = if double_quote { ("3 2 (First)", "7 4 (Second)", "\"") } else { ("(First)", "(Second)", "'") };
                let prefix = format!("BT /F1 12 Tf 18 TL 72 700 Td {earlier} {first}");
                let body = format!("{quote} {second} {quote}");
                let tail = "1 0 0 rg (A A) ' ET";
                let mut doc = text_page("");
                // Pin the tail's advances explicitly instead of relying on the editor's
                // approximate standard-font metrics when checking retained Tc and Tw.
                doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
                    let mut widths = vec![Object::Int(500); 34];
                    widths[0] = Object::Int(278);
                    widths[33] = Object::Int(667);
                    font.set(b"FirstChar".to_vec(), Object::Int(32));
                    font.set(b"LastChar".to_vec(), Object::Int(65));
                    font.set(b"Widths".to_vec(), Object::Array(widths));
                })
                .unwrap();
                let refs = if separate_pieces {
                    set_content_pieces(&mut doc, &[&prefix, &body, tail])
                } else {
                    set_content_pieces(&mut doc, &[&format!("{prefix} {body} {tail}")])
                };
                let blocks = text::text_blocks(&doc, 0).unwrap();
                let target = blocks.iter().position(|b| b.text == "First Second").unwrap();
                text::replace_block(&mut doc, 0, target, "Edited").unwrap();
                if separate_pieces {
                    assert_eq!(content_refs(&doc)[2], refs[2], "the untouched tail keeps its original object");
                }
                let doc = reopen(&doc);
                let lines = text::text_lines(&doc, 0).unwrap();
                assert!(!lines.iter().any(|l| l.text.contains("First") || l.text.contains("Second")));
                let edited = lines.iter().find(|l| l.text == "Edited").unwrap();
                assert_eq!(edited.origin_baseline(), 682.0);
                let tail = lines.iter().find(|l| l.text == "A A").unwrap();
                // Three quote moves of 18 pt from y=700 put the unedited tail at 646,
                // including when reflow split the original BT after Earlier.
                assert_eq!(tail.origin_baseline(), 646.0, "split={split_text_object}, pieces={separate_pieces}, double={double_quote}");
                assert_eq!(tail.rect[0], 72.0);
                // Helvetica A=667, space=278 units. The last double quote leaves
                // Tc=4 for three characters and Tw=7 for the one space.
                let expected_width = (667.0 * 2.0 + 278.0) * 12.0 / 1000.0 + if double_quote { 3.0 * 4.0 + 7.0 } else { 0.0 };
                assert!((tail.rect[2] - tail.rect[0] - expected_width).abs() < 0.001, "{:?}", tail.rect);
                if split_text_object {
                    assert_eq!(lines.iter().find(|l| l.text == "Earlier").unwrap().origin_baseline(), 700.0);
                }
            }
        }
    }
}

#[test]
fn standard_text_substitutes_reserve_inherited_font_names() {
    for (mode, preferred, base) in [(0, "PCEdHelvetica", "Helvetica"), (1, "PCEdHelv", "Helvetica"), (2, "PCEdTimesRoman", "Times-Roman")] {
        let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
        let legacy = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), vec![])));
        let names = [preferred.to_string(), format!("{preferred}_")];
        let imported = install_legacy_font_content(&mut doc, legacy, &names);
        // The shared inherited dictionary is another preservation boundary.
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let shared = doc.add(page.dict.get(b"Resources").unwrap().clone());
        let original_resources = doc.get(shared);
        doc.update_dict(pdfcraft_cos::ObjRef::new(2, 0), |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
        doc.update_dict(page.obj, |d| {
            d.remove(b"Resources");
        })
        .unwrap();
        // An imported stream already has writer-derived Length metadata. Establish
        // that persisted baseline before comparing the complete untouched object.
        let mut doc = reopen(&doc);
        let original_stream = doc.get(legacy);
        match mode {
            0 => {
                text::replace_line(&mut doc, 0, 0, "abc").unwrap();
            }
            1 => {
                text::replace_block(&mut doc, 0, 0, "abc").unwrap();
            }
            _ => {
                text::rewrite_block(
                    &mut doc,
                    0,
                    0,
                    Some("abc"),
                    &text::BlockStyle { family: Some((crate::added::Family::Times, false, false)), ..Default::default() },
                )
                .unwrap();
            }
        }
        let doc = reopen(&doc);
        let fonts = page_fonts(&doc, &pdfcraft_model::pages(&doc)[0]);
        for name in names {
            assert_eq!(fonts.get(name.as_bytes()), Some(&imported));
        }
        let generated = doc.resolve(fonts.get(format!("{preferred}__").as_bytes()).unwrap());
        assert_eq!(generated.as_dict().unwrap().name(b"BaseFont"), Some(base.as_bytes()));
        assert_eq!(doc.get(legacy), original_stream, "untouched legacy stream keeps its object and bytes");
        assert_eq!(doc.get(shared), original_resources, "inherited resources are copied before editing");
        let lines = text::text_lines(&doc, 0).unwrap();
        let legacy = lines.iter().find(|line| line.text == "Legacy text").unwrap();
        assert_eq!(legacy.base_font, "Courier-Bold");
        assert_eq!((legacy.rect[0], legacy.origin_baseline()), (50.0, 300.0));
        assert!((legacy.rect[2] - legacy.rect[0] - 79.2).abs() < 0.001, "fixed Courier advance");
        assert!(lines.iter().any(|line| line.text == "abc" && line.base_font == base));
    }
}

#[test]
fn line_replacement_preserves_later_quote_effects_and_tail_streams() {
    for first_is_quote in [false, true] {
        for double_quote in [false, true] {
            for separate_pieces in [false, true] {
                let quote = if double_quote { "\"" } else { "'" };
                let first_operand = if first_is_quote && double_quote { "0 0 (I)" } else { "(I)" };
                let first_op = if first_is_quote { quote } else { "Tj" };
                let second_operand = if double_quote { "7 4 (I)" } else { "(I)" };
                // A 2pt leading keeps both small I runs in one editable line. The later
                // quote's movement and spacing still determine the untouched red tail.
                let prefix = format!("BT /F1 12 Tf 2 TL 72 700 Td {first_operand}");
                let body = format!("{first_op} {second_operand} {quote}");
                let tail = "1 0 0 rg 18 TL (A A) ' ET";
                let mut doc = text_page("");
                doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
                    let mut widths = vec![Object::Int(500); 42];
                    widths[0] = Object::Int(278);
                    widths[33] = Object::Int(667);
                    widths[41] = Object::Int(260);
                    font.set(b"FirstChar".to_vec(), Object::Int(32));
                    font.set(b"LastChar".to_vec(), Object::Int(73));
                    font.set(b"Widths".to_vec(), Object::Array(widths));
                })
                .unwrap();
                let refs = if separate_pieces {
                    set_content_pieces(&mut doc, &[&prefix, &body, tail])
                } else {
                    set_content_pieces(&mut doc, &[&format!("{prefix} {body} {tail}")])
                };
                let lines = text::text_lines(&doc, 0).unwrap();
                let target = lines.iter().position(|line| line.text == "II").unwrap();
                assert_eq!(text::replace_line(&mut doc, 0, target, "A").unwrap().substituted, None);
                if separate_pieces {
                    assert_eq!(content_refs(&doc)[2], refs[2]);
                }
                let doc = reopen(&doc);
                let lines = text::text_lines(&doc, 0).unwrap();
                assert!(!lines.iter().any(|line| line.text.contains('I')));
                let tail = lines.iter().find(|line| line.text == "A A").unwrap();
                assert_eq!(tail.rect[0], 72.0);
                assert_eq!(tail.origin_baseline(), if first_is_quote { 678.0 } else { 680.0 });
                let expected = 19.344 + if double_quote { 19.0 } else { 0.0 };
                assert!((tail.rect[2] - tail.rect[0] - expected).abs() < 0.001, "fixed advances plus retained Tc/Tw");
            }
        }
    }
}

fn install_raw_text_font_names(doc: &mut Document, font: pdfcraft_cos::ObjRef) {
    doc.update_dict(pdfcraft_cos::ObjRef::new(3, 0), |page| {
        let mut resources = page.get(b"Resources").unwrap().as_dict().unwrap().clone();
        let mut fonts = resources.get(b"Font").unwrap().as_dict().unwrap().clone();
        for name in [vec![b'F', 0x80], vec![b'F', 0x81]] {
            fonts.set(name, Object::Ref(font));
        }
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
}

#[test]
fn raw_font_names_survive_line_reuse_and_split_paragraph_restoration() {
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F#80 12 Tf 72 750 Td 1 0 0 rg (Earlier) Tj 0 -50 Td 0 g (Target) Tj 0 -50 Td 1 0 0 rg (Tail) Tj ET");
        install_raw_text_font_names(&mut doc, pdfcraft_cos::ObjRef::new(5, 0));
        let result = if paragraph {
            let blocks = text::text_blocks(&doc, 0).unwrap();
            let target = blocks.iter().position(|block| block.text == "Target").unwrap();
            text::replace_block(&mut doc, 0, target, "Edited").unwrap()
        } else {
            let lines = text::text_lines(&doc, 0).unwrap();
            let target = lines.iter().position(|line| line.text == "Target").unwrap();
            text::replace_line(&mut doc, 0, target, "Edited").unwrap()
        };
        assert_eq!(result.substituted, None, "raw identity resolves the original font");
        let doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["Earlier", "Edited", "Tail"]);
        assert_eq!(lines[0].origin_baseline(), 750.0);
        assert_eq!(lines[2].origin_baseline(), 650.0);
        let parsed = pdfcraft_content::parse(&page_content_bytes(&doc, 0));
        for op in parsed.ops.iter().filter(|op| op.is("Tf")) {
            assert_eq!(op.name(0), Some(&[b'F', 0x80][..]), "all restored names retain their exact PDF bytes");
        }
    }
}

#[test]
fn lossy_display_labels_never_merge_distinct_font_resources() {
    let mut doc = text_page("BT /F#80 12 Tf 72 700 Td (One) Tj /F#81 12 Tf 0 -18 Td (Two) Tj ET");
    install_raw_text_font_names(&mut doc, pdfcraft_cos::ObjRef::new(5, 0));
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines[0].font, lines[1].font, "public labels can be the same lossy string");
    assert_eq!(text::text_blocks(&doc, 0).unwrap().len(), 2, "raw identities keep paragraphs separate");
    for paragraph in [false, true] {
        let mut doc = text_page("BT /F#80 12 Tf 72 700 Td (A) Tj ET BT /F#81 12 Tf 72 700 Td (A) Tj ET");
        install_raw_text_font_names(&mut doc, pdfcraft_cos::ObjRef::new(5, 0));
        if paragraph {
            text::replace_block(&mut doc, 0, 0, "B").unwrap();
        } else {
            text::replace_line(&mut doc, 0, 0, "B").unwrap();
        }
        let lines = text::text_lines(&reopen(&doc), 0).unwrap();
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["B", "A"], "the other resource's coincident text survives");
    }
}

#[test]
fn raw_font_restoration_and_unsupported_line_refusal_are_atomic() {
    let mut doc = text_page("BT /F#80 12 Tf 72 700 Td (ab) Tj 1 0 0 rg 0 -30 Td (ab) Tj ET");
    install_raw_text_font_names(&mut doc, pdfcraft_cos::ObjRef::new(6, 0));
    assert_eq!(text::replace_line(&mut doc, 0, 0, "abc").unwrap().substituted.as_deref(), Some("Helvetica"));
    let mut doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    let tail = lines.iter().find(|line| line.text == "ab").unwrap();
    assert_eq!((tail.base_font.as_str(), tail.rect[0], tail.origin_baseline()), ("ABCDEF+Arial", 72.0, 670.0));
    assert!((tail.rect[2] - tail.rect[0] - 12.24).abs() < 0.001);
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(text::replace_line(&mut doc, 0, 0, "Ω").is_err());
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn removed_coincident_quote_copies_keep_their_nonpainting_state() {
    for double_quote in [false, true] {
        let duplicate = if double_quote { "7 1 (A) \"" } else { "(A) '" };
        let mut doc = text_page("");
        doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
            let mut widths = vec![Object::Int(500); 34];
            widths[0] = Object::Int(278);
            widths[33] = Object::Int(667);
            font.set(b"FirstChar".to_vec(), Object::Int(32));
            font.set(b"LastChar".to_vec(), Object::Int(65));
            font.set(b"Widths".to_vec(), Object::Array(widths));
        })
        .unwrap();
        let refs = set_content_pieces(
            &mut doc,
            &["BT /F1 12 Tf 72 700 Td (A) Tj ET", &format!("BT /F1 12 Tf 18 TL 72 718 Td {duplicate}"), "1 0 0 rg (A A) ' ET"],
        );
        text::replace_line(&mut doc, 0, 0, "A A").unwrap();
        assert_eq!(content_refs(&doc)[2], refs[2]);
        let doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 2, "one replacement and the untouched tail; the coincident copy is removed");
        let tail = lines.iter().find(|line| line.color == [1.0, 0.0, 0.0]).unwrap();
        assert_eq!((tail.rect[0], tail.origin_baseline()), (72.0, 682.0));
        let expected = 19.344 + if double_quote { 10.0 } else { 0.0 };
        assert!((tail.rect[2] - tail.rect[0] - expected).abs() < 0.001);
    }
}

#[test]
fn flattening_reserves_existing_xobject_names() {
    use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation};
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut legacy = Vec::new();
    let mut xobjects = Dict::new();
    for name in ["PCFl0", "PCFl0_"] {
        let mut form = Dict::new();
        form.set(b"Type".to_vec(), Object::name("XObject"));
        form.set(b"Subtype".to_vec(), Object::name("Form"));
        form.set(b"BBox".to_vec(), Object::Array([0, 0, 10, 10].map(Object::Int).to_vec()));
        form.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        let object = doc.add(Object::Stream(Stream::from_raw(form, b"0 0 10 10 re f\n".to_vec())));
        xobjects.set(name.as_bytes().to_vec(), Object::Ref(object));
        legacy.push((name, Object::Ref(object)));
    }
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    let original = b"q /PCFl0 Do /PCFl0_ Do Q\n";
    let stream = doc.add(Object::Stream(Stream::from_raw(Dict::new(), original.to_vec())));
    doc.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Ref(stream));
    })
    .unwrap();
    let shape = Shape::Rectangle { rect: [20.0, 20.0, 40.0, 40.0] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "ordinary rectangle".into(), author: "Fixture".into() },
        &Meta { date: None, id: "flatten-collision".into() },
    )
    .unwrap();
    assert_eq!(flatten(&mut doc, &[0, 0], true, false).unwrap(), 1, "a repeated page must not redraw a removed annotation");
    let doc = reopen(&doc);
    let page = &pdfcraft_model::pages(&doc)[0];
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let xobjects = xobjects.as_dict().unwrap();
    for (name, object) in legacy {
        assert_eq!(xobjects.get(name.as_bytes()), Some(&object), "{name} must retain its original object");
    }
    assert!(xobjects.contains(b"PCFl0__"));
    assert!(!xobjects.contains(b"PCFl1"), "the duplicate page must retain only the first appearance binding");
    let content = streams(&doc, 0);
    assert!(content.iter().any(|s| s.as_bytes() == original), "original drawing bytes must stay unchanged");
    assert!(content.last().unwrap().contains("/PCFl0__ Do"));
    assert!(!page.dict.contains(b"Annots"));
}

#[test]
fn raw_stamps_reserve_standard_fonts_without_rewriting_other_bytes() {
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
    let mut legacy = Vec::new();
    for (name, base) in [("PCHelv", "Courier"), ("PCHelv_", "Courier"), ("PCTimes", "Helvetica"), ("PCCour", "Times-Roman")] {
        let font = standard_font(base);
        fonts.set(name.as_bytes().to_vec(), font.clone());
        legacy.push((name, font));
    }
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let original = b"BT /PCHelv 12 Tf (Legacy) Tj /PCTimes 10 Tf (Times) Tj /PCCour 9 Tf (Courier) Tj ET\n";
    let stream = doc.add(Object::Stream(Stream::from_raw(Dict::new(), original.to_vec())));
    doc.update_dict(page.obj, |d| {
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Ref(stream));
    })
    .unwrap();
    let stamp_bytes = b"% /PCHelv 88 Tf stays a comment\nBT /PCHelv % keep spacing\n12 Tf (/PCTimes 77 Tf stays a string) Tj \
        /PC#48elv 11 Tf (Escaped name) Tj /PCTimes 10 Tf (Times) Tj /PCCour 9 Tf (Courier) Tj ET\n\
        BI /W 1 /H 1 /BPC 8 /CS /G ID x EI\n42 UnknownOp\n";
    let expected = b"% /PCHelv 88 Tf stays a comment\nBT /PCHelv__ % keep spacing\n12 Tf (/PCTimes 77 Tf stays a string) Tj \
        /PCHelv__ 11 Tf (Escaped name) Tj /PCTimes_ 10 Tf (Times) Tj /PCCour_ 9 Tf (Courier) Tj ET\n\
        BI /W 1 /H 1 /BPC 8 /CS /G ID x EI\n42 UnknownOp\n";
    stamp(&mut doc, 0, "RawCollision", stamp_bytes.to_vec()).unwrap();
    let boxes = b"0 0 10 10 re f\n";
    stamp(&mut doc, 0, "Boxes", boxes.to_vec()).unwrap();
    let doc = reopen(&doc);
    let page = &pdfcraft_model::pages(&doc)[0];
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
    let fonts = fonts.as_dict().unwrap();
    for (name, font) in legacy {
        assert_eq!(fonts.get(name.as_bytes()), Some(&font), "{name} must retain its imported font");
    }
    for (name, base) in [("PCHelv__", "Helvetica"), ("PCTimes_", "Times-Roman"), ("PCCour_", "Courier")] {
        assert_eq!(doc.resolve(fonts.get(name.as_bytes()).unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(base.as_bytes()));
    }
    assert_eq!(fonts.len(), 8, "repeated stamps reuse identical reserved fonts");
    let content = streams(&doc, 0);
    assert!(content.iter().any(|s| s.as_bytes() == original));
    assert!(content.iter().any(|s| s.as_bytes() == expected), "only Tf font-name tokens may change: {content:?}");
    assert_eq!(content.last().unwrap().as_bytes(), boxes, "non-text raw stamp bytes must stay exact");

    // Each escaped alias must work by itself, without an unescaped copy elsewhere in the bytes.
    for (name, escaped, base, imported) in [
        ("PCHelv", "PC#48elv", "Helvetica", "Courier"),
        ("PCTimes", "PC#54imes", "Times-Roman", "Helvetica"),
        ("PCCour", "PC#43our", "Courier", "Times-Roman"),
    ] {
        let mut doc = fixture();
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let mut resources = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
        let mut fonts = doc.resolve(resources.get(b"Font").unwrap()).as_dict().unwrap().clone();
        let legacy = standard_font(imported);
        fonts.set(name.as_bytes().to_vec(), legacy.clone());
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
        let bytes = format!("BT /{escaped} 10 Tf (X) Tj ET\n");
        stamp(&mut doc, 0, "EscapedFont", bytes.into_bytes()).unwrap();
        let doc = reopen(&doc);
        let page = &pdfcraft_model::pages(&doc)[0];
        let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
        let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
        let fonts = fonts.as_dict().unwrap();
        assert_eq!(fonts.get(name.as_bytes()), Some(&legacy));
        let alias = format!("{name}_");
        assert_eq!(doc.resolve(fonts.get(alias.as_bytes()).unwrap()).as_dict().unwrap().name(b"BaseFont"), Some(base.as_bytes()));
        assert_eq!(fonts.len(), 3, "only the requested standard font may be installed");
        assert_eq!(streams(&doc, 0).last().unwrap(), &format!("BT /{alias} 10 Tf (X) Tj ET\n"));
    }

    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    let before = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap()).as_dict().unwrap().clone();
    let bytes = b"% /PCHelv /PCTimes /PCCour\nBX (/PCHelv /PCTimes /PCCour) UnknownOp EX\nq 0 0 10 10 re f Q\n";
    stamp(&mut doc, 0, "NoFont", bytes.to_vec()).unwrap();
    let doc = reopen(&doc);
    let page = &pdfcraft_model::pages(&doc)[0];
    let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
    assert_eq!(doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap()).as_dict(), Some(&before));
    assert_eq!(streams(&doc, 0).last().unwrap().as_bytes(), bytes, "comments and strings do not request fonts");
}

#[test]
fn paragraph_reflow_preserves_same_baseline_tail_advances_and_next_line_origin() {
    // Original standard-font dictionaries with explicit advances: all printable
    // glyphs are 600 units, except the 200-unit space. No font assets are needed.
    for earlier in [false, true] {
        for pieces in [false, true] {
            for (show, advance, quoted, char_spacing) in [
                ("(A A) Tj", 22.4, false, 3.0),
                ("[(A) 125 ( ) -50 (A)] TJ", 21.8, false, 3.0),
                ("(A A) '", 22.4, true, 3.0),
                ("7 4 (A A) \"", 26.4, true, 4.0),
            ] {
                let prefix = format!("BT /F1 10 Tf 3 Tc 5 Tw 80 Tz 18 TL 1 0 0 1 72 700 Tm {}", if earlier { "0 0 1 rg (EE) Tj 0 g" } else { "" });
                let tail = "1 0 0 rg (Tail) Tj 0 1 0 rg (Next) ' ET";
                let mut doc = text_page("");
                doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
                    let mut widths = vec![Object::Int(600); 95];
                    widths[0] = Object::Int(200);
                    font.set(b"FirstChar".to_vec(), Object::Int(32));
                    font.set(b"LastChar".to_vec(), Object::Int(126));
                    font.set(b"Widths".to_vec(), Object::Array(widths));
                })
                .unwrap();
                let refs = if pieces {
                    set_content_pieces(&mut doc, &[&prefix, show, tail])
                } else {
                    set_content_pieces(&mut doc, &[&format!("{prefix} {show} {tail}")])
                };
                let before = text::text_lines(&doc, 0).unwrap();
                let blocks = text::text_blocks(&doc, 0).unwrap();
                let target = blocks.iter().position(|block| block.text == "A A").unwrap();
                let style = text::BlockStyle { color: Some([0.2, 0.2, 0.8]), ..Default::default() };
                text::rewrite_block(&mut doc, 0, target, None, &style).unwrap();
                if pieces {
                    assert_eq!(content_refs(&doc)[2], refs[2], "the untouched tail stream retains its object");
                }
                let saved = reopen(&doc);
                let after = text::text_lines(&saved, 0).unwrap();
                let expected = if earlier { vec!["EE", "A A", "Tail", "Next"] } else { vec!["A A", "Tail", "Next"] };
                assert_eq!(after.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), expected);
                assert_eq!(text::text_blocks(&saved, 0).unwrap().iter().map(|block| block.text.as_str()).collect::<Vec<_>>(), expected);
                let tail = after.iter().find(|line| line.text == "Tail").unwrap();
                let expected_x = 72.0 + if earlier && !quoted { 14.4 } else { 0.0 } + advance;
                assert!((tail.rect[0] - expected_x).abs() < 0.001, "earlier={earlier}, pieces={pieces}, show={show}: {:?}", tail.rect);
                assert_eq!(tail.color, [1.0, 0.0, 0.0]);
                assert_eq!(tail.origin_baseline(), if quoted { 682.0 } else { 700.0 });
                assert!((tail.rect[2] - tail.rect[0] - 4.0 * (6.0 + char_spacing) * 0.8).abs() < 0.001);
                for label in ["Tail", "Next"] {
                    let old = before.iter().find(|line| line.text == label).unwrap();
                    let new = after.iter().find(|line| line.text == label).unwrap();
                    assert!(close(old.rect, new.rect), "untouched {label} moved: {:?} -> {:?}", old.rect, new.rect);
                    assert_eq!(old.color, new.color);
                }
                let next = after.iter().find(|line| line.text == "Next").unwrap();
                assert_eq!(next.rect[0], 72.0, "TJ compensation must not change the line matrix");
                assert_eq!(next.origin_baseline(), if quoted { 664.0 } else { 682.0 });
                if earlier {
                    let old = before.iter().find(|line| line.text == "EE").unwrap();
                    let new = after.iter().find(|line| line.text == "EE").unwrap();
                    assert!(close(old.rect, new.rect));
                }
                assert_eq!(after.iter().filter(|line| line.text == "A A").count(), 1, "the old text must not remain invisibly");
            }
        }
    }
}

#[test]
fn paragraph_advance_refusal_preserves_unsupported_vertical_source() {
    let mut doc = text_page("BT /F1 10 Tf 1 0 0 1 72 700 Tm <0041> Tj 1 0 0 rg <0054> Tj ET");
    let cmap = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"2 beginbfchar <0041> <0041> <0054> <0054> endbfchar".to_vec())));
    let mut descendant = Dict::new();
    descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    descendant.set(b"DW".to_vec(), Object::Int(600));
    doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
        font.set(b"Subtype".to_vec(), Object::name("Type0"));
        font.set(b"Encoding".to_vec(), Object::name("Identity-V"));
        font.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)]));
        font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    })
    .unwrap();
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let style = text::BlockStyle { color: Some([0.0, 0.0, 1.0]), ..Default::default() };
    let error = text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap_err();
    assert!(error.to_string().contains("vertical"), "{error}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn nonpainting_advances_keep_real_spaces_and_undecodable_runs() {
    let spaced = text_page("BT /F1 10 Tf 72 700 Td (A) Tj [( )] TJ (A) Tj ET");
    assert_eq!(text::text_lines(&spaced, 0).unwrap()[0].text, "A A");
    // Unknown Unicode must not make a nonempty shown string disappear from the
    // line's decodability contract. A named custom glyph need not have an AGL name.
    let mut unknown = text_page("BT /F1 10 Tf 72 700 Td (A) Tj [(B)] TJ (A) Tj ET");
    unknown
        .update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
            let mut encoding = Dict::new();
            encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
            encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(66), Object::name("CustomGlyph")]));
            font.set(b"Encoding".to_vec(), Object::Dict(encoding));
            font.set(b"FirstChar".to_vec(), Object::Int(65));
            font.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(600), Object::Int(600)]));
        })
        .unwrap();
    let lines = text::text_lines(&unknown, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "AA");
    assert!(!lines[0].decodable);
}

#[test]
fn paragraph_standard_face_layout_uses_the_emitted_times_bold_advances() {
    for (text, expected) in [("OOOOOOOO", vec!["OOOOOOOO"]), ("OOOOOO OOOOOO", vec!["OOOOOO", "OOOOOO"])] {
        let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Original) Tj ET");
        let style = text::BlockStyle {
            family: Some((added::Family::Times, true, false)),
            size: Some(12.0),
            width: Some(100.0),
            align: Some(added::Align::Right),
            underline: Some(true),
            ..Default::default()
        };
        text::rewrite_block(&mut doc, 0, 0, Some(text), &style).unwrap();
        let doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), expected);
        for (i, line) in lines.iter().enumerate() {
            // Independent standard Times-Bold O=778, not the approximate Helvetica
            // measure previously used for choosing line breaks and alignment offsets.
            let advance = line.text.len() as f64 * 0.778 * 12.0;
            assert_eq!(line.base_font, "Times-Bold");
            assert!((line.rect[0] - (172.0 - advance)).abs() < 0.001);
            assert!((line.rect[2] - 172.0).abs() < 0.001);
            assert!((line.origin_baseline() - (700.0 - i as f64 * 14.4)).abs() < 0.001);
        }
        let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
        let shown: Vec<_> = ops.iter().filter(|op| op.is("Tj")).map(|op| op.operands[0].as_string().unwrap().bytes.clone()).collect();
        assert_eq!(shown, expected.iter().map(|line| line.as_bytes().to_vec()).collect::<Vec<_>>());
        let ends: Vec<_> = ops.iter().filter(|op| op.is("l")).map(|op| op.num(0).unwrap()).collect();
        assert_eq!(ends, vec![100.0; expected.len()], "underlines end at the same text-space right edge");
    }
}

#[test]
fn paragraph_layout_uses_output_spacing_while_untouched_tail_keeps_source_spacing() {
    for justify in [false, true] {
        let mut doc = text_page("BT /F1 10 Tf 2 Tc 5 Tw 80 Tz 18 TL 72 700 Td (A A) Tj 1 0 0 rg (T T) Tj 0 1 0 rg (Next) ' ET");
        doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
            let mut widths = vec![Object::Int(600); 95];
            widths[0] = Object::Int(200);
            font.set(b"FirstChar".to_vec(), Object::Int(32));
            font.set(b"Widths".to_vec(), Object::Array(widths));
        })
        .unwrap();
        let style = text::BlockStyle {
            width: Some(if justify { 30.0 } else { 100.0 }),
            align: Some(if justify { added::Align::Justify } else { added::Align::Right }),
            color: Some([0.0, 0.0, 1.0]),
            underline: Some(true),
            ..Default::default()
        };
        text::rewrite_block(&mut doc, 0, 0, Some(if justify { "A A A A A" } else { "A A" }), &style).unwrap();
        let doc = reopen(&doc);
        let lines = text::text_lines(&doc, 0).unwrap();
        let expected = if justify { vec!["A A A", "A A", "T T", "Next"] } else { vec!["A A", "T T", "Next"] };
        assert_eq!(lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), expected);
        let first = &lines[0];
        // Emitted A A: (6 + 2 + 2 + 2 + 6 + 2) * 0.8 = 16pt, with Tw reset
        // to zero. A A A is 25.6pt before justification; two byte32 spaces need
        // Tw=(30-25.6)/0.8/2=2.75 to reach the requested 30pt line width.
        let (left, right) = if justify { (72.0, 102.0) } else { (156.0, 172.0) };
        assert!((first.rect[0] - left).abs() < 0.001);
        assert!((first.rect[2] - right).abs() < 0.001);
        if justify {
            assert!((lines[1].rect[0] - 72.0).abs() < 0.001);
            assert!((lines[1].rect[2] - 88.0).abs() < 0.001);
            assert_eq!(lines[1].origin_baseline(), 682.0);
        }
        let tail = lines.iter().find(|line| line.text == "T T").unwrap();
        assert_eq!(tail.color, [1.0, 0.0, 0.0]);
        assert!((tail.rect[0] - 92.0).abs() < 0.001, "original A A advance stays 20pt");
        assert!((tail.rect[2] - 112.0).abs() < 0.001, "untouched tail retains Tc2/Tw5/Tz80");
        let next = lines.iter().find(|line| line.text == "Next").unwrap();
        assert_eq!((next.rect[0], next.origin_baseline()), (72.0, 682.0));
        let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
        if justify {
            assert!(ops.iter().any(|op| op.is("Tw") && op.num(0) == Some(2.75)));
        }
        let ends: Vec<_> = ops.iter().filter(|op| op.is("l")).map(|op| op.num(0).unwrap()).collect();
        assert_eq!(ends, if justify { vec![30.0, 16.0] } else { vec![100.0] });
    }
}

#[test]
fn paragraph_custom_space_encoding_aligns_exact_bytes_and_refuses_unsupported_justification() {
    // Byte32 selects A, while byte65 selects space. Tw therefore expands A in the
    // source, not the Unicode spaces. Reflow must not carry that Tw into its measure.
    let mut doc = text_page("BT /F1 10 Tf 2 Tc 5 Tw 80 Tz 72 700 Td ( A ) Tj ET");
    doc.update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
        let mut encoding = Dict::new();
        encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(32), Object::name("A"), Object::Int(65), Object::name("space")]));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        let mut widths = vec![Object::Int(600); 95];
        widths[33] = Object::Int(200);
        font.set(b"FirstChar".to_vec(), Object::Int(32));
        font.set(b"Widths".to_vec(), Object::Array(widths));
    })
    .unwrap();
    let style = text::BlockStyle { width: Some(100.0), align: Some(added::Align::Right), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap();
    let mut doc = reopen(&doc);
    let lines = text::text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "A A");
    assert!((lines[0].rect[0] - 156.0).abs() < 0.001);
    assert!((lines[0].rect[2] - 172.0).abs() < 0.001);
    let ops = pdfcraft_content::parse(&page_content_bytes(&doc, 0)).ops;
    let shown: Vec<_> = ops.iter().filter(|op| op.is("Tj")).map(|op| op.operands[0].as_string().unwrap().bytes.clone()).collect();
    assert_eq!(shown, vec![b" A ".to_vec()], "custom bytes survive save/reopen");
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let style = text::BlockStyle { width: Some(30.0), align: Some(added::Align::Justify), ..Default::default() };
    let error = text::rewrite_block(&mut doc, 0, 0, Some("A A A A A"), &style).unwrap_err();
    assert!(error.to_string().contains("outside byte 32"), "{error}");
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before, "unsupported justification changes nothing");
}
