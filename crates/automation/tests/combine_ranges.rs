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

fn delivery_session_state(a: &Automation) -> Value {
    json!(
        a.session()
            .docs()
            .iter()
            .map(|doc| json!({
                "id": doc.id.0, "name": doc.name, "pages": doc.info.pages.len(), "bytes": doc.bytes.as_slice(), "path": doc.path,
                "dirty": doc.dirty, "undo": doc.can_undo(), "redo": doc.can_redo(),
            }))
            .collect::<Vec<_>>()
    )
}

fn delivery_texts(a: &mut Automation, doc: &Value) -> Vec<String> {
    call(a, "text_extract", json!({ "doc": doc }))["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|page| page["text"].as_str().unwrap().trim().to_string())
        .collect()
}

#[test]
fn delivery_boolean_errors_preserve_inputs_destinations_and_session_before_io() {
    let dir = ScratchDir::new("delivery-boolean-preflight");
    let first = fixture("First");
    let second = fixture("Second");
    std::fs::write(dir.join("first.pdf"), &first).unwrap();
    std::fs::write(dir.join("second.pdf"), &second).unwrap();
    let sentinel = b"existing output survives invalid delivery options";
    std::fs::write(dir.join("combined.pdf"), sentinel).unwrap();
    std::fs::create_dir(dir.join("parts")).unwrap();
    std::fs::write(dir.join("parts/first-page1.pdf"), sentinel).unwrap();
    let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path":"first.pdf"}))["doc"].clone();
    call(&mut a, "doc_set_info", json!({"doc":doc,"key":"Title","value":"Unsaved source state"}));
    let session = delivery_session_state(&a);
    for invalid in [json!("yes"), json!(1), json!([]), json!({})] {
        for out in ["combined.pdf", "not-created.pdf"] {
            let error = a.call("doc_combine", &json!({"paths":["first.pdf","second.pdf"],"out":out,"open":invalid})).unwrap_err();
            assert_eq!(error, ToolError::InvalidArgs("open must be true or false".into()));
            for separate in [false, true] {
                for key in ["open", "delete"] {
                    let mut args = json!({"doc":doc,"pages":[1],"out":out,"out_dir":"parts","open":true,"delete":false,"separate":separate});
                    args[key] = invalid.clone();
                    let error = a.call("page_extract", &args).unwrap_err();
                    assert_eq!(error, ToolError::InvalidArgs(format!("{key} must be true or false")));
                    args["out_dir"] = json!("not-created-parts");
                    assert!(matches!(a.call("page_extract", &args), Err(ToolError::InvalidArgs(_))));
                    assert!(!dir.join("not-created-parts").exists());
                    assert_eq!(delivery_session_state(&a), session);
                    assert_eq!(std::fs::read(dir.join("combined.pdf")).unwrap(), sentinel);
                    assert_eq!(std::fs::read(dir.join("parts/first-page1.pdf")).unwrap(), sentinel);
                    assert!(!dir.join("not-created.pdf").exists());
                    assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), first);
                    assert_eq!(std::fs::read(dir.join("second.pdf")).unwrap(), second);
                }
            }
        }
    }
    let error = a.call("doc_combine", &json!({"paths":["missing-a.pdf","missing-b.pdf"],"out":"not-created.pdf","open":"yes"})).unwrap_err();
    assert_eq!(error, ToolError::InvalidArgs("open must be true or false".into()), "delivery validation precedes input-file reads");
    assert_eq!(delivery_session_state(&a), session);
    assert!(!dir.join("not-created.pdf").exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);
    assert_eq!(std::fs::read_dir(dir.join("parts")).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn extract_all_distinct_pages_with_delete_refuses_before_output_or_session_changes() {
    let dir = ScratchDir::new("delivery-delete-preflight");
    let input = fixture("First");
    std::fs::write(dir.join("first.pdf"), &input).unwrap();
    let sentinel = b"existing extraction output";
    std::fs::write(dir.join("extract.pdf"), sentinel).unwrap();
    std::fs::create_dir(dir.join("parts")).unwrap();
    for page in 1..=2 {
        std::fs::write(dir.join(format!("parts/first-page{page}.pdf")), sentinel).unwrap();
    }
    let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path":"first.pdf"}))["doc"].clone();
    call(&mut a, "doc_set_info", json!({"doc":doc,"key":"Title","value":"Unsaved source state"}));
    let session = delivery_session_state(&a);
    let expected = ToolError::Failed("a document must keep at least one page".into());
    assert_eq!(a.call("page_delete", &json!({"doc":doc,"pages":[1,2]})).unwrap_err(), expected);
    for pages in [json!([1, 2]), json!([2, 1, 1])] {
        for separate in [false, true] {
            for (out, out_dir) in [("extract.pdf", "parts"), ("not-created.pdf", "not-created-parts")] {
                let args = json!({"doc":doc,"pages":pages,"delete":true,"open":true,"separate":separate,"out":out,"out_dir":out_dir});
                assert_eq!(a.call("page_extract", &args).unwrap_err(), expected);
                assert_eq!(delivery_session_state(&a), session);
                assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), input);
                assert_eq!(std::fs::read(dir.join("extract.pdf")).unwrap(), sentinel);
                for page in 1..=2 {
                    assert_eq!(std::fs::read(dir.join(format!("parts/first-page{page}.pdf"))).unwrap(), sentinel);
                }
                assert!(!dir.join("not-created.pdf").exists());
                assert!(!dir.join("not-created-parts").exists());
            }
        }
    }
    let error = a.call("page_extract", &json!({"doc":doc,"pages":[3],"delete":true,"out":"not-created.pdf"})).unwrap_err();
    assert!(matches!(error, ToolError::InvalidArgs(ref message) if message.contains("out of range")));
    assert_eq!(delivery_session_state(&a), session);
    assert!(!dir.join("not-created.pdf").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn valid_delivery_defaults_and_duplicate_extraction_delete_keep_saved_contents() {
    let dir = ScratchDir::new("delivery-valid-controls");
    let first = fixture("First");
    let second = fixture("Second");
    std::fs::write(dir.join("first.pdf"), &first).unwrap();
    std::fs::write(dir.join("second.pdf"), &second).unwrap();
    let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
    let all = ["First page 1", "First page 2", "Second page 1", "Second page 2"];
    for (out, open, should_open) in [
        (Some("combined.pdf"), None, false),
        (Some("combined.pdf"), Some(Value::Null), false),
        (Some("combined.pdf"), Some(json!(false)), false),
        (Some("combined.pdf"), Some(json!(true)), true),
        (None, None, true),
        (None, Some(json!(false)), false),
    ] {
        let before = a.session().docs().len();
        let mut args = json!({"paths":["first.pdf","second.pdf"]});
        if let Some(out) = out {
            args["out"] = json!(out);
        }
        if let Some(open) = open {
            args["open"] = open;
        }
        let result = call(&mut a, "doc_combine", args);
        assert_eq!(a.session().docs().len(), before + usize::from(should_open));
        assert_eq!(result.get("document").is_some(), should_open);
        if should_open {
            assert_eq!(delivery_texts(&mut a, &result["document"]["doc"]), all);
        }
        if let Some(out) = out {
            let reopened = call(&mut a, "doc_open", json!({"path":out}));
            assert_eq!(delivery_texts(&mut a, &reopened["doc"]), all);
        }
    }
    for separate in [false, true] {
        let doc = call(&mut a, "doc_open", json!({"path":"first.pdf"}))["doc"].clone();
        let before = a.session().docs().len();
        let result = call(
            &mut a,
            "page_extract",
            json!({"doc":doc,"pages":[1,1],"delete":true,"open":true,"separate":separate,"out":"extract.pdf","out_dir":"parts"}),
        );
        assert_eq!(a.session().docs().len(), before + usize::from(!separate));
        assert_eq!(delivery_texts(&mut a, &doc), ["First page 2"]);
        if separate {
            assert_eq!(result["files"].as_array().unwrap().len(), 2);
            let reopened = call(&mut a, "doc_open", json!({"path":result["files"][0]}));
            assert_eq!(delivery_texts(&mut a, &reopened["doc"]), ["First page 1"]);
        } else {
            let reopened = call(&mut a, "doc_open", json!({"path":"extract.pdf"}));
            assert_eq!(delivery_texts(&mut a, &reopened["doc"]), ["First page 1", "First page 1"]);
            assert_eq!(delivery_texts(&mut a, &result["document"]["doc"]), ["First page 1", "First page 1"]);
        }
        assert_eq!(result["original"]["pages"], 1);
        assert_eq!(call(&mut a, "edit_undo", json!({"doc":doc}))["document"]["pages"], 2);
        assert_eq!(delivery_texts(&mut a, &doc), ["First page 1", "First page 2"]);
    }
    let doc = call(&mut a, "doc_open", json!({"path":"first.pdf"}))["doc"].clone();
    let result = call(&mut a, "page_extract", json!({"doc":doc,"pages":[2]}));
    assert_eq!(delivery_texts(&mut a, &result["document"]["doc"]), ["First page 2"]);
    assert_eq!(delivery_texts(&mut a, &doc), ["First page 1", "First page 2"]);
    let before = a.session().docs().len();
    let result = call(&mut a, "page_extract", json!({"doc":doc,"pages":[2],"out":"default-extract.pdf"}));
    assert!(result.get("document").is_none());
    assert_eq!(a.session().docs().len(), before);
    let reopened = call(&mut a, "doc_open", json!({"path":"default-extract.pdf"}));
    assert_eq!(delivery_texts(&mut a, &reopened["doc"]), ["First page 2"]);
    assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), first);
    assert_eq!(std::fs::read(dir.join("second.pdf")).unwrap(), second);
    std::fs::remove_dir_all(dir).unwrap();
}
