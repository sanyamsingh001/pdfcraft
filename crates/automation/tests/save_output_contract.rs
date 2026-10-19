//! Contributor-original normal-operation regressions for save/output contracts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn fixture() -> Vec<u8> {
    let mut objects =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 300] >>".into()];
    for content in 7..=9 {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Resources << /Font << /Helv 6 0 R >> >> /Contents {content} 0 R >>"));
    }
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for text in ["First source page", "Second source page", "Third source page"] {
        let content = format!("BT /Helv 12 Tf 20 260 Td ({text}) Tj ET");
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn workdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tmp").join(format!("save-output-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("book.pdf"), fixture()).unwrap();
    dir.canonicalize().unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().into_iter().next().unwrap() {
        Content::Json(value) => value,
        _ => panic!("{tool}: expected JSON"),
    }
}

fn strategy(result: &Value, expected: &str) {
    assert_eq!(result["strategy"], expected);
    assert_eq!(result["incremental"], expected == "incremental");
    assert_eq!(result["document"]["dirty"], false);
}

fn reopened(path: &Path, password: Option<&str>) -> pdfcraft_cos::Document {
    pdfcraft_cos::Document::open_with_password(Arc::new(std::fs::read(path).unwrap()), password).unwrap()
}

#[test]
fn bookmark_split_collisions_keep_each_part_and_preserve_unique_preferred_names() {
    let dir = workdir("split");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let cases = [
        (["Intro", "Intro", "Intro-part2"], ["book-Intro.pdf", "book-Intro-part2_.pdf", "book-Intro-part2.pdf"]),
        (["A/B", "A?B", "A_B-part2"], ["book-A_B.pdf", "book-A_B-part2_.pdf", "book-A_B-part2.pdf"]),
        (["Report", "report", "report-part2"], ["book-Report.pdf", "book-report-part2_.pdf", "book-report-part2.pdf"]),
        (["One", "Two", "Three"], ["book-One.pdf", "book-Two.pdf", "book-Three.pdf"]),
    ];
    for (case, (titles, expected)) in cases.into_iter().enumerate() {
        let doc = call(&mut a, "doc_open", json!({ "path": "book.pdf" }))["doc"].as_u64().unwrap();
        for (page, title) in titles.iter().enumerate() {
            call(&mut a, "bookmark_add", json!({ "doc": doc, "page": page + 1, "title": title }));
        }
        for attempt in 0..2 {
            let result = call(&mut a, "doc_split", json!({ "doc": doc, "bookmarks": true, "out_dir": format!("split-{case}-{attempt}") }));
            let files = result["files"].as_array().unwrap();
            assert_eq!(files.len(), 3);
            assert_eq!(files.iter().map(|f| f["path"].as_str().unwrap()).collect::<HashSet<_>>().len(), 3);
            for (page, (file, name)) in files.iter().zip(expected).enumerate() {
                let path = Path::new(file["path"].as_str().unwrap());
                assert_eq!(path.file_name().unwrap().to_str().unwrap(), name);
                assert_eq!(file["first_page"], page + 1);
                assert_eq!(file["last_page"], page + 1);
                let opened = call(&mut a, "doc_open", json!({ "path": path }));
                assert_eq!(opened["pages"], 1);
                let part = opened["doc"].as_u64().unwrap();
                let extracted = call(&mut a, "text_extract", json!({ "doc": part }));
                let text = extracted["pages"][0]["text"].as_str().unwrap();
                assert!(text.contains(["First", "Second", "Third"][page]), "{text}");
                for other in ["First", "Second", "Third"].into_iter().filter(|t| *t != ["First", "Second", "Third"][page]) {
                    assert!(!text.contains(other), "an overwritten part must not masquerade as its predecessor: {text}");
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn save_results_report_unchanged_incremental_and_required_full_rewrites() {
    let dir = workdir("strategy");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let original = std::fs::read(dir.join("book.pdf")).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "book.pdf" }))["doc"].as_u64().unwrap();
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc })), "unchanged");
    assert_eq!(std::fs::read(dir.join("book.pdf")).unwrap(), original);
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Normal edit" }));
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "full": false })), "incremental");
    assert!(std::fs::read(dir.join("book.pdf")).unwrap().starts_with(&original));
    assert_eq!(reopened(&dir.join("book.pdf"), None).revisions().len(), 2);
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc })), "unchanged");
    call(&mut a, "doc_remove_hidden", json!({ "doc": doc, "categories": ["metadata"] }));
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "full": false })), "full");
    assert_eq!(reopened(&dir.join("book.pdf"), None).revisions().len(), 1);
    call(&mut a, "doc_protect", json!({ "doc": doc, "open_password": "original-test-password", "compatibility": "aes-128" }));
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "full": false })), "full");
    assert_eq!(reopened(&dir.join("book.pdf"), Some("original-test-password")).revisions().len(), 1);
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc })), "unchanged");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn new_and_reconstructed_documents_report_the_writer_strategy() {
    let dir = workdir("new-repaired");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let new = call(&mut a, "doc_create", json!({ "from": "blank" }))["doc"].as_u64().unwrap();
    strategy(&call(&mut a, "doc_save", json!({ "doc": new, "path": "new.pdf" })), "full");
    assert_eq!(reopened(&dir.join("new.pdf"), None).revisions().len(), 1);
    strategy(&call(&mut a, "doc_save", json!({ "doc": new, "path": "copy.pdf", "full": false })), "unchanged");
    assert_eq!(std::fs::read(dir.join("new.pdf")).unwrap(), std::fs::read(dir.join("copy.pdf")).unwrap());
    let original = fixture();
    let xref = original.windows(b"xref\n".len()).position(|w| w == b"xref\n").unwrap();
    // Original ordinary page objects with an absent xref table: the normal repair path.
    let repaired = original[..xref].to_vec();
    std::fs::write(dir.join("repaired.pdf"), &repaired).unwrap();
    assert!(reopened(&dir.join("repaired.pdf"), None).revisions().is_empty());
    let doc = call(&mut a, "doc_open", json!({ "path": "repaired.pdf" }))["doc"].as_u64().unwrap();
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Repaired original" }));
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "full": false })), "full");
    assert_eq!(reopened(&dir.join("repaired.pdf"), None).revisions().len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn unconfined_tool_outputs_update_alias_targets_without_replacing_links() {
    let dir = workdir("unconfined-alias");
    let alias = dir.join("alias.pdf");
    std::os::unix::fs::symlink("book.pdf", &alias).unwrap();
    let mut a = Automation::new();
    let doc = call(&mut a, "doc_open", json!({ "path": alias }))["doc"].as_u64().unwrap();
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Saved through alias" }));
    let saved = call(&mut a, "doc_save", json!({ "doc": doc }));
    strategy(&saved, "incremental");
    assert_eq!(Path::new(saved["path"].as_str().unwrap()), alias);
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    let target = call(&mut a, "doc_open", json!({ "path": dir.join("book.pdf") }))["doc"].as_u64().unwrap();
    assert_eq!(call(&mut a, "doc_info", json!({ "doc": target }))["title"], "Saved through alias");
    let text_alias = dir.join("text-alias.txt");
    std::fs::write(dir.join("text.txt"), b"old text").unwrap();
    std::os::unix::fs::symlink("text.txt", &text_alias).unwrap();
    call(&mut a, "text_extract", json!({ "doc": doc, "path": text_alias }));
    assert!(text_alias.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(std::fs::read_to_string(dir.join("text.txt")).unwrap().contains("First source page"));
    let extract_alias = dir.join("extract-alias.pdf");
    std::os::unix::fs::symlink("subset.pdf", &extract_alias).unwrap();
    call(&mut a, "page_extract", json!({ "doc": doc, "pages": [2], "out": extract_alias, "open": false }));
    assert!(extract_alias.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(call(&mut a, "doc_open", json!({ "path": dir.join("subset.pdf") }))["pages"], 1);
    std::fs::create_dir(dir.join("folder")).unwrap();
    let invalid = dir.join("folder-alias.pdf");
    std::os::unix::fs::symlink("folder", &invalid).unwrap();
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Unsaved edit" }));
    assert!(a.call("doc_save", &json!({ "doc": doc, "path": invalid })).is_err());
    assert!(invalid.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(call(&mut a, "doc_info", json!({ "doc": doc }))["document"]["dirty"], true);
    let nested = dir.join("new-folder/nested/saved.pdf");
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "path": nested })), "full");
    assert_eq!(reopened(&nested, None).revisions().len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn confined_alias_saves_preserve_links_and_refuse_outside_targets() {
    let dir = workdir("confined-alias");
    let root = dir.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("inside.pdf"), fixture()).unwrap();
    let alias = root.join("alias.pdf");
    std::os::unix::fs::symlink("inside.pdf", &alias).unwrap();
    let mut a = Automation::new().with_root(&root).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "alias.pdf" }))["doc"].as_u64().unwrap();
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Inside save" }));
    strategy(&call(&mut a, "doc_save", json!({ "doc": doc, "path": "alias.pdf" })), "incremental");
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    let inside = call(&mut a, "doc_open", json!({ "path": "inside.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(call(&mut a, "doc_info", json!({ "doc": inside }))["title"], "Inside save");
    let outside = dir.join("book.pdf");
    let before = std::fs::read(&outside).unwrap();
    let escape = root.join("outside-alias.pdf");
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    call(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Still unsaved" }));
    assert!(a.call("doc_save", &json!({ "doc": doc, "path": "outside-alias.pdf" })).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), before);
    assert!(escape.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(call(&mut a, "doc_info", json!({ "doc": doc }))["document"]["dirty"], true);
    std::fs::remove_dir_all(dir).unwrap();
}
