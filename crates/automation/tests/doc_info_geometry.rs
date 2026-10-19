//! Document summaries and the MCP info resource use displayed-page geometry (#129).

#[path = "support/scratch.rs"]
mod scratch;
use scratch::ScratchDir;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Contributor-original blank page with nonzero crop origin, rotation and nondefault UserUnit.
fn fixture(rotation: i32) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [10 20 622 812] /CropBox [30 40 582 752] /Rotate {rotation} /UserUnit 2 /Resources << >> >>"),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON, got {other:?}"),
    }
}

fn rect_close(actual: &Value, expected: &Value) {
    let actual = actual.as_array().unwrap();
    let expected = expected.as_array().unwrap();
    assert_eq!(actual.len(), 4);
    assert_eq!(expected.len(), 4);
    for (a, b) in actual.iter().zip(expected) {
        assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.02, "{actual:?} != {expected:?}");
    }
}

#[test]
fn doc_info_and_mcp_rectangles_match_list_tools_and_can_be_reused() {
    for rotation in [0, 90, 180, 270] {
        let dir = ScratchDir::new(&format!("doc-info-geometry-{rotation}"));
        std::fs::write(dir.join("input.pdf"), fixture(rotation)).unwrap();
        let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
        let doc = call(&mut a, "doc_open", json!({"path":"input.pdf"}))["doc"].clone();
        call(&mut a, "comment_add", json!({"doc":doc,"page":1,"type":"note","at":[72,72],"author":"Fixture","contents":"Synthetic note"}));
        call(&mut a, "link_add", json!({"doc":doc,"page":1,"rect":[72,100,200,120],"url":"https://example.com"}));
        call(&mut a, "doc_save", json!({"doc":doc,"path":"saved.pdf"}));
        let doc = call(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].clone();
        let info = call(&mut a, "doc_info", json!({"doc":doc}));
        let comments = call(&mut a, "comment_list", json!({"doc":doc}));
        let links = call(&mut a, "link_list", json!({"doc":doc}));
        let note = info["annotations"].as_array().unwrap().iter().find(|item| item["type"] == "Text").unwrap();
        rect_close(&note["rect"], &comments["comments"][0]["rect"]);
        rect_close(&info["links"][0]["rect"], &links["links"][0]["rect"]);
        rect_close(&info["links"][0]["rect"], &json!([72, 100, 200, 120]));
        call(&mut a, "link_edit", json!({"doc":doc,"page":1,"index":links["links"][0]["index"],"rect":info["links"][0]["rect"]}));
        let after = call(&mut a, "link_list", json!({"doc":doc}));
        rect_close(&after["links"][0]["rect"], &links["links"][0]["rect"]);

        #[cfg(feature = "mcp")]
        {
            use pdfcraft_automation::mcp::McpServer;
            let mut server = McpServer::new(a);
            let request = json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":format!("pdfcraft://doc/{doc}/info")}}).to_string();
            let reply: Value = serde_json::from_str(&server.handle_line(&request).unwrap()).unwrap();
            let resource: Value = serde_json::from_str(reply["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(resource["annotations"], info["annotations"]);
            assert_eq!(resource["links"], info["links"]);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
