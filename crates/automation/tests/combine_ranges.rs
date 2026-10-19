//! Contributor-original two-input checks for combine range arguments and saved contents.
#[path = "support/scratch.rs"]
mod scratch;
use scratch::ScratchDir;

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn fixture(label: &str) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 300] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F 5 0 R >> >> /Contents 6 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F 5 0 R >> >> /Contents 7 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for page in 1..=2 {
        let content = format!("BT /F 12 Tf 20 260 Td ({label} page {page}) Tj ET");
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 8\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[test]
fn combine_rejects_invalid_range_elements_before_output_and_preserves_valid_selection() {
    let dir = ScratchDir::new("combine-range-contract");
    let first = fixture("First");
    let second = fixture("Second");
    std::fs::write(dir.join("first.pdf"), &first).unwrap();
    std::fs::write(dir.join("second.pdf"), &second).unwrap();
    let sentinel = b"existing destination must survive invalid selectors";
    std::fs::write(dir.join("combined.pdf"), sentinel).unwrap();
    let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
    for invalid in [json!(1), json!(true), json!({"page":1}), json!(["1"])] {
        for pages in [json!([invalid, null]), json!([null, invalid])] {
            for out in ["combined.pdf", "not-created.pdf"] {
                let error = a.call("doc_combine", &json!({"paths":["first.pdf","second.pdf"],"pages":pages,"out":out,"open":false})).unwrap_err();
                assert!(matches!(error, ToolError::InvalidArgs(ref message) if message.contains("range string or null")), "{error:?}");
                assert_eq!(std::fs::read(dir.join("combined.pdf")).unwrap(), sentinel);
                assert!(!dir.join("not-created.pdf").exists());
                assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), first);
                assert_eq!(std::fs::read(dir.join("second.pdf")).unwrap(), second);
            }
        }
    }
    // Even missing input files cannot supersede a malformed selector's argument error.
    let error = a.call("doc_combine", &json!({"paths":["missing-a.pdf","missing-b.pdf"],"pages":[false,null],"out":"not-created.pdf"})).unwrap_err();
    assert!(matches!(error, ToolError::InvalidArgs(_)));
    assert!(!dir.join("not-created.pdf").exists());

    let all = vec!["First page 1", "First page 2", "Second page 1", "Second page 2"];
    for (pages, expected) in [
        (None, all.clone()),
        (Some(Value::Null), all.clone()),
        (Some(json!([null, null])), all.clone()),
        (Some(json!(["", null])), all),
        (Some(json!(["2", null])), vec!["First page 2", "Second page 1", "Second page 2"]),
        (Some(json!([null, "1"])), vec!["First page 1", "First page 2", "Second page 1"]),
    ] {
        let mut args = json!({"paths":["first.pdf","second.pdf"],"out":"combined.pdf","open":false});
        if let Some(pages) = pages {
            args["pages"] = pages;
        }
        let combined = call(&mut a, "doc_combine", args);
        assert!(combined["bytes"].as_u64().unwrap() > 0);
        let opened = call(&mut a, "doc_open", json!({"path":"combined.pdf"}));
        assert_eq!(opened["pages"].as_u64().unwrap(), expected.len() as u64);
        let extracted = call(&mut a, "text_extract", json!({"doc":opened["doc"]}));
        let actual: Vec<_> = extracted["pages"].as_array().unwrap().iter().map(|page| page["text"].as_str().unwrap().trim()).collect();
        assert_eq!(actual, expected);
        assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), first);
        assert_eq!(std::fs::read(dir.join("second.pdf")).unwrap(), second);
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3, "no staging files or failed outputs remain");
    std::fs::remove_dir_all(dir).unwrap();
}
