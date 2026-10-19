use super::*;
use pdfcraft_cos::{SaveOptions, Stream, write_full};
use std::sync::Arc;

// Contributor-original one-page PDF using the standard Courier face. The optional
// contents array exercises text state across stream boundaries without font assets.
fn original(parts: &[&str]) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), pdfcraft_fonts::generated::standard_font("Courier"));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let contents = parts.iter().map(|part| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), part.as_bytes().to_vec()))))).collect();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 600, 800].map(Object::Int).to_vec()));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Array(contents));
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

// Independent saved-operator oracle: Tr survives BT/ET and belongs to q/Q state.
// It deliberately does not use text_lines, Origin, TextState or interpret.
fn shown_modes(doc: &Document) -> Vec<(String, i64)> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let ops = Content::read(doc, &page.dict).unwrap().ops;
    let mut mode = 0;
    let mut stack = Vec::new();
    let mut shown = Vec::new();
    for op in ops {
        match op.op.as_slice() {
            b"q" => stack.push(mode),
            b"Q" => mode = stack.pop().unwrap(),
            b"Tr" => mode = op.operands[0].as_int().unwrap(),
            b"Tj" => shown.push((String::from_utf8(op.operands[0].as_string().unwrap().bytes.clone()).unwrap(), mode)),
            b"TJ" => assert!(op.operands[0].as_array().unwrap().iter().all(|value| value.as_f64().is_some()), "only nonpainting compensation"),
            _ => {}
        }
    }
    assert!(stack.is_empty());
    shown
}

#[test]
fn nonfill_paragraph_reflow_refuses_without_changing_saved_content() {
    for mode in 1..=7 {
        for outside_bt in [false, true] {
            let prefix = if outside_bt { format!("{mode} Tr BT") } else { format!("BT {mode} Tr") };
            let mut doc = original(&[&prefix, "/F1 12 Tf 0.25 w 1 0 0 RG 1 0 0 1 72 700 Tm (Original) Tj ET"]);
            assert_eq!(shown_modes(&doc), [("Original".into(), mode)]);
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            let objects = doc.object_numbers();
            let error = rewrite_block(&mut doc, 0, 0, Some("Changed"), &BlockStyle::default()).unwrap_err();
            assert!(error.to_string().contains("non-fill text rendering"), "{error}");
            assert_eq!(doc.object_numbers(), objects);
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert_eq!(shown_modes(&saved(&doc)), [("Original".into(), mode)]);
        }
    }
}

#[test]
fn nonfill_line_replacement_keeps_the_source_rendering_mode() {
    for mode in [1, 3] {
        for outside_bt in [false, true] {
            let prefix = if outside_bt { format!("{mode} Tr BT") } else { format!("BT {mode} Tr") };
            let mut doc = original(&[&prefix, "/F1 12 Tf 0.25 w 1 0 0 RG 1 0 0 1 72 700 Tm (Original) Tj ET"]);
            assert_eq!(replace_line(&mut doc, 0, 0, "Changed").unwrap().substituted, None);
            let doc = saved(&doc);
            assert_eq!(shown_modes(&doc), [("Changed".into(), mode)]);
            let page = pdfcraft_model::pages(&doc).remove(0);
            let ops = Content::read(&doc, &page.dict).unwrap().ops;
            assert!(ops.iter().any(|op| op.is("w") && op.nums::<1>() == Some([0.25])));
            assert!(ops.iter().any(|op| op.is("RG") && op.nums::<3>() == Some([1.0, 0.0, 0.0])));
        }
    }
}

#[test]
fn nonfill_line_type3_substitution_refuses_before_allocating_a_fallback_font() {
    for mode in [1, 3] {
        let content = format!("BT /F1 12 Tf {mode} Tr 72 700 Td (Original) Tj ET");
        let mut doc = original(&[&content]);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        let objects = doc.object_numbers();
        let error = replace_line(&mut doc, 0, 0, "日").unwrap_err();
        assert!(error.to_string().contains("Type3 fallback cannot preserve a non-fill"), "{error}");
        assert_eq!(doc.object_numbers(), objects);
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        assert_eq!(shown_modes(&saved(&doc)), [("Original".into(), mode)]);
    }
}

#[test]
fn rendering_modes_separate_coincident_copies_for_line_and_paragraph_edits() {
    for mode in [1, 3] {
        for paragraph in [false, true] {
            let content = format!("BT /F1 12 Tf 0 Tr 72 700 Td (Same) Tj {mode} Tr 1 0 0 1 72 700 Tm (Same) Tj ET");
            let mut doc = original(&[&content]);
            assert_eq!(text_lines(&doc, 0).unwrap().len(), 2);
            if paragraph {
                rewrite_block(&mut doc, 0, 0, Some("Edited"), &BlockStyle::default()).unwrap();
            } else {
                replace_line(&mut doc, 0, 0, "Edited").unwrap();
            }
            assert_eq!(shown_modes(&saved(&doc)), [("Edited".into(), 0), ("Same".into(), mode)]);
        }
        // Editing the non-fill copy must likewise keep the coincident filled copy.
        let content = format!("BT /F1 12 Tf {mode} Tr 72 700 Td (Same) Tj 0 Tr 1 0 0 1 72 700 Tm (Same) Tj ET");
        let mut doc = original(&[&content]);
        replace_line(&mut doc, 0, 0, "Edited").unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("Edited".into(), mode), ("Same".into(), 0)]);
    }
}

#[test]
fn rendering_modes_separate_adjacent_runs_and_paragraphs() {
    for mode in [1, 3] {
        let content = format!("BT /F1 12 Tf 72 700 Td (First) Tj {mode} Tr (Second) Tj ET");
        let mut doc = original(&[&content]);
        assert_eq!(text_lines(&doc, 0).unwrap().iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        replace_line(&mut doc, 0, 1, "Changed").unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("First".into(), 0), ("Changed".into(), mode)]);

        let content = format!("BT /F1 12 Tf 72 700 Td (First) Tj {mode} Tr 0 -14 Td (Second) Tj ET");
        let mut doc = original(&[&content]);
        assert_eq!(text_blocks(&doc, 0).unwrap().iter().map(|block| block.text.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        rewrite_block(&mut doc, 0, 0, Some("Edited"), &BlockStyle::default()).unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("Edited".into(), 0), ("Second".into(), mode)]);
    }
}

#[test]
fn filled_paragraphs_keep_explicit_reset_and_graphics_state_restoration() {
    for prefix in ["BT", "0 Tr BT", "3 Tr BT 0 Tr", "q 3 Tr Q BT"] {
        let mut doc = original(&[prefix, "/F1 12 Tf 72 700 Td (Original) Tj ET"]);
        rewrite_block(&mut doc, 0, 0, Some("Changed"), &BlockStyle::default()).unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("Changed".into(), 0)], "{prefix}");
    }
    // BT does not reset Tr. A non-fill value inherited from a preceding text
    // object must still refuse, whereas a q/Q-scoped mode must not leak outward.
    let mut doc = original(&["BT /F1 12 Tf 3 Tr 72 740 Td (Earlier) Tj ET", "BT 72 700 Td (Later) Tj ET"]);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let error = rewrite_block(&mut doc, 0, 1, Some("Changed"), &BlockStyle::default()).unwrap_err();
    assert!(error.to_string().contains("non-fill text rendering"), "{error}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    assert_eq!(shown_modes(&saved(&doc)), [("Earlier".into(), 3), ("Later".into(), 3)]);
}

#[test]
fn filled_paragraph_after_pending_text_clip_refuses_without_committing_the_clip_early() {
    for mode in 4..=7 {
        for earlier in ["Mask", " "] {
            let content = format!("BT /F1 12 Tf {mode} Tr 72 700 Td ({earlier}) Tj 0 Tr 0 -14 Td (Visible) Tj ET");
            let mut doc = original(&[&content]);
            let target = text_blocks(&doc, 0).unwrap().iter().position(|block| block.text == "Visible").unwrap();
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            let objects = doc.object_numbers();
            let error = rewrite_block(&mut doc, 0, target, Some("Changed"), &BlockStyle::default()).unwrap_err();
            assert!(error.to_string().contains("earlier text clip too soon"), "{error}");
            assert_eq!(doc.object_numbers(), objects);
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert_eq!(shown_modes(&saved(&doc)), [(earlier.into(), mode), ("Visible".into(), 0)]);
            // Editing in place does not insert an ET and remains supported.
            let line = text_lines(&doc, 0).unwrap().iter().position(|line| line.text == "Visible").unwrap();
            replace_line(&mut doc, 0, line, "Changed").unwrap();
            let doc = saved(&doc);
            assert_eq!(shown_modes(&doc), [(earlier.into(), mode), ("Changed".into(), 0)]);
            let page = pdfcraft_model::pages(&doc).remove(0);
            let ops = Content::read(&doc, &page.dict).unwrap().ops;
            assert_eq!(ops.iter().filter(|op| op.is("ET")).count(), 1);
            assert!(ops.last().unwrap().is("ET"), "the original text clip still commits only at the end");
        }
    }
    for mode in [1, 3] {
        let content = format!("BT /F1 12 Tf {mode} Tr 72 700 Td (Earlier) Tj 0 Tr 0 -14 Td (Visible) Tj ET");
        let mut doc = original(&[&content]);
        rewrite_block(&mut doc, 0, 1, Some("Changed"), &BlockStyle::default()).unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("Earlier".into(), mode), ("Changed".into(), 0)]);
    }
}

#[test]
fn completed_text_clips_do_not_block_later_filled_paragraphs() {
    for mode in 4..=7 {
        let mut doc = original(&[&format!("BT /F1 12 Tf {mode} Tr 72 700 Td (Mask) Tj ET"), "BT 0 Tr 72 700 Td (Visible) Tj ET"]);
        rewrite_block(&mut doc, 0, 1, Some("Changed"), &BlockStyle::default()).unwrap();
        assert_eq!(shown_modes(&saved(&doc)), [("Mask".into(), mode), ("Changed".into(), 0)]);
    }
}
