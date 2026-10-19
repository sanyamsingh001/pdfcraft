//! Synthetic regression for text editing when /Contents splits an operator's operands.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn fixture() -> Vec<u8> {
    let stream = |body: &str| format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len());
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".to_string(),
        stream("/P << /MCID 0"),
        stream(">> BDC BT /F1 12 Tf 72 700 Td (Target) Tj 0 -20 Td [(After) -20 (wards)]"),
        stream("TJ ET EMC"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let result = a.call(tool, &args).unwrap_or_else(|error| panic!("{tool}: {error}"));
    let Content::Json(value) = &result[0] else { panic!("{tool} should return JSON") };
    value.clone()
}

#[test]
fn split_text_can_be_listed_edited_styled_undone_and_saved_through_tools() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-split-content-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("split.pdf"), fixture()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "split.pdf" }))["doc"].as_u64().unwrap();
    let before = call(&mut a, "text_lines", json!({ "doc": doc, "page": 1 }));
    assert_eq!(before["count"], 2);
    assert_eq!(before["lines"][1]["text"], "Afterwards");
    call(&mut a, "text_edit", json!({ "doc": doc, "page": 1, "line": 2, "text": "Edited" }));
    let edited = call(&mut a, "text_lines", json!({ "doc": doc, "page": 1 }));
    assert_eq!(edited["lines"][0], before["lines"][0]);
    assert_eq!(edited["lines"][1]["text"], "Edited");
    call(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(call(&mut a, "text_lines", json!({ "doc": doc, "page": 1 })), before);
    call(&mut a, "edit_redo", json!({ "doc": doc }));
    let paragraphs = call(&mut a, "text_paragraphs", json!({ "doc": doc, "page": 1 }));
    let index = paragraphs["paragraphs"].as_array().unwrap().iter().position(|p| p["text"] == "Edited").unwrap() + 1;
    call(&mut a, "text_edit", json!({ "doc": doc, "page": 1, "paragraph": index, "color": "#ff0000" }));
    call(&mut a, "doc_save", json!({ "doc": doc, "path": "edited.pdf" }));
    let reopened = call(&mut a, "doc_open", json!({ "path": "edited.pdf" }))["doc"].as_u64().unwrap();
    let saved = call(&mut a, "text_lines", json!({ "doc": reopened, "page": 1 }));
    assert_eq!(saved["count"], 2);
    assert_eq!(saved["lines"][0], before["lines"][0]);
    assert_eq!(saved["lines"][1]["text"], "Edited");
    let rendered = a.call("page_render", &json!({ "doc": reopened, "page": 1, "dpi": 72 })).unwrap();
    let Content::Png { data, width, height } = &rendered[0] else { panic!("page_render should return PNG") };
    assert_eq!((*width, *height), (600, 800));
    assert!(data.starts_with(b"\x89PNG\r\n\x1a\n"));
    std::fs::remove_dir_all(dir).unwrap();
}
