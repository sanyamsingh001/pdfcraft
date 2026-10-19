//! Password-tool documentation describes the existing conditional permission policy (#134).

#[path = "support/scratch.rs"]
mod scratch;
use scratch::ScratchDir;

use pdfcraft_automation::{Automation, Content, tools};
use serde_json::{Value, json};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[test]
fn protection_permissions_and_defaults_apply_only_with_a_permissions_password() {
    let def = tools().into_iter().find(|tool| tool.name == "doc_protect").unwrap();
    assert!(def.description.contains("only when permissions_password is supplied"));
    assert!(def.description.contains("open_password alone allows all operations"));
    for field in ["printing", "changes", "copy", "accessibility"] {
        assert!(def.input_schema["properties"][field]["description"].as_str().unwrap().contains("permissions_password"));
    }
    let dir = ScratchDir::new("protection-contract");
    let mut a = Automation::new().with_root(dir.as_ref()).unwrap();
    for (name, mut args, restricted) in [
        ("open-only.pdf", json!({"open_password":"openme"}), false),
        ("open-explicit.pdf", json!({"open_password":"openme","copy":false,"changes":"none","printing":"none","accessibility":false}), false),
        ("owner-defaults.pdf", json!({"open_password":"openme","permissions_password":"ownerme"}), true),
    ] {
        let doc = call(&mut a, "doc_create", json!({"from":"blank"}))["doc"].clone();
        args["doc"] = doc.clone();
        call(&mut a, "doc_protect", args);
        call(&mut a, "doc_save", json!({"doc":doc,"path":name}));
        let mut reopened = Automation::new().with_root(dir.as_ref()).unwrap();
        assert!(reopened.call("doc_open", &json!({"path":name})).is_err(), "all variants require the opening password");
        let doc = call(&mut reopened, "doc_open", json!({"path":name,"password":"openme"}))["doc"].clone();
        let info = call(&mut reopened, "doc_info", json!({"doc":doc}));
        let security = &info["security"];
        assert_eq!(security["protected"], true);
        assert_eq!(security["copy"], !restricted);
        assert_eq!(security["modify"], !restricted);
        assert_eq!(security["printing"], "high");
        assert_eq!(security["accessibility"], true);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
