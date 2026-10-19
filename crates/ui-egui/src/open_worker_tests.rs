use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use crate::{PdfCraftApp, control::ControlClient, open_worker};

fn fixture(pages: usize) -> Vec<u8> {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids = (0..pages).map(|page| format!("{} 0 R", 4 + 2 * page)).collect::<Vec<_>>().join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} /MediaBox [0 0 200 300] >>"));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for page in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * page));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", page + 1);
        objects.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let saved = slot.clone();
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language preference");
        *saved.lock().expect("control-client slot") = Some(app.attach_control(&creation.egui_ctx));
        app.open_bytes("existing.pdf", None, fixture(3)).expect("existing document");
        app
    });
    harness.run_steps(3);
    (harness, slot.lock().expect("control-client slot").take().expect("control client"))
}

#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn request(harness: &mut Harness<'static, PdfCraftApp>, client: &ControlClient, method: &str, params: Value) -> Value {
    let reply = client.send(method, params);
    for _ in 0..5 {
        harness.step();
        if let Ok(result) = reply.try_recv() {
            return result.expect("control request succeeded");
        }
    }
    panic!("{method} was not answered during the held prepare phase");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn open_prepare_keeps_control_and_existing_tabs_live_until_atomic_commit() {
    let (mut harness, client) = harness();
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);

    let path = std::env::temp_dir().join(format!("pdfcraft-open-worker-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture(2)).expect("write test PDF");
    let open_reply = request(&mut harness, &client, "ui.open", json!({ "path": path.to_string_lossy() }));
    assert_eq!(open_reply["pending"], true);
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");

    let state = request(&mut harness, &client, "ui.state", json!({}));
    assert_eq!(state["documents"].as_array().map(Vec::len), Some(1));
    assert_eq!(state["documents"][0]["name"], "existing.pdf");
    assert_eq!(state["pending_opens"].as_array().map(Vec::len), Some(1));
    request(&mut harness, &client, "ui.command", json!({ "id": "page.insert_blank" }));
    assert_eq!(harness.state().session.get(harness.state().views[0].id).expect("existing tab").info.pages.len(), 4);
    assert_eq!(harness.state().views.len(), 1, "prepared state is not visible in the session");

    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
    harness.step();
    let app = harness.state();
    assert_eq!(app.views.len(), 2, "one prepared document commits exactly once");
    assert_eq!(app.session.get(app.views[1].id).expect("new tab").info.pages.len(), 2);
    assert_eq!(app.active, Some(1));
    let _ = std::fs::remove_file(path);
}

struct ToolFile {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
}

#[allow(clippy::expect_used)]
impl ToolFile {
    fn new(case: &str, name: &str, bytes: &[u8]) -> Self {
        let directory = std::path::PathBuf::from(".tmp").join(format!("pdfcraft-deferred-tool-{}-{case}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("create original fixture directory");
        let path = directory.join(name);
        std::fs::write(&path, bytes).expect("write original fixture");
        Self { directory, path }
    }

    fn picked_path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for ToolFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[allow(clippy::expect_used)]
fn tool_harness(picks: Vec<String>, inline: bool) -> Harness<'static, PdfCraftApp> {
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |_creation| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language preference");
        app.pick_override = Some(picks);
        app.run_inline = inline;
        app
    });
    harness.run_steps(2);
    harness
}

#[allow(clippy::expect_used)]
fn finish_tool_prepare(
    harness: &mut Harness<'static, PdfCraftApp>,
    started: &std::sync::mpsc::Receiver<()>,
    release: &std::sync::mpsc::Sender<()>,
    finished: &std::sync::mpsc::Receiver<()>,
) {
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker completed preparation");
    harness.step();
}

#[test]
#[allow(clippy::expect_used)]
fn tool_waits_for_async_open_and_runs_once_on_the_picked_pdf() {
    let original = fixture(2);
    let file = ToolFile::new("pdf", "picked.pdf", &original);
    let mut harness = tool_harness(vec![file.picked_path()], false);
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    harness.state_mut().run_command("export.text");
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
    assert_eq!(harness.state().pending_opens.len(), 1);
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());
    harness.run_steps(2);
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none(), "the held open must not run the tool");
    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker completed preparation");
    harness.step();
    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.active, Some(0));
    let doc = app.session.get(app.views[0].id).expect("picked document");
    assert_eq!(doc.path.as_deref(), Some(file.picked_path().as_str()));
    assert_eq!(doc.info.pages.len(), 2);
    assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
    assert_eq!(doc.saved_bytes().as_slice(), original);
    assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))), "the original tool runs after admission");
    harness.state_mut().dialog = None;
    harness.run_steps(2);
    assert!(harness.state().dialog.is_none(), "the completed continuation is consumed once");
    assert_eq!(harness.state().views.len(), 1);
    assert_eq!(std::fs::read(&file.path).expect("read original PDF"), original);
}

#[allow(clippy::expect_used)]
fn protected_tool_fixture() -> Arc<Vec<u8>> {
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("plain.pdf", None, Arc::new(fixture(1)), None).expect("open original PDF");
    session
        .apply(
            id,
            pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
                open_password: Some("secret".into()),
                permissions_password: Some("owner".into()),
                copy: true,
                ..Default::default()
            }),
        )
        .expect("protect original PDF");
    session.save_bytes(id).expect("save protected fixture")
}

#[test]
#[allow(clippy::expect_used)]
fn tool_continuation_survives_password_retry_and_is_dropped_on_cancel() {
    let original = protected_tool_fixture();
    for cancel in [false, true] {
        let file = ToolFile::new(if cancel { "password-cancel" } else { "password-retry" }, "protected.pdf", &original);
        let mut harness = tool_harness(vec![file.picked_path()], false);
        let (gate, started, release, finished) = open_worker::prepare_gate();
        harness.state_mut().open_prepare_gate = Some(gate);
        harness.state_mut().run_command("export.text");
        harness.step();
        finish_tool_prepare(&mut harness, &started, &release, &finished);
        let prompt = harness.state().password_prompt.as_ref().expect("password prompt");
        assert_eq!(prompt.path.as_deref(), Some(file.picked_path().as_str()));
        assert_eq!(prompt.then, Some("export.text"));
        assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());
        if cancel {
            harness.state_mut().submit_password(None);
            harness.run_steps(2);
            assert!(harness.state().password_prompt.is_none() && harness.state().pending_opens.is_empty());
            assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());
            harness.state_mut().open_bytes("later.pdf", None, fixture(1)).expect("open unrelated later document");
            assert!(harness.state().dialog.is_none(), "cancelled continuation cannot run on a later file");
        } else {
            harness.state_mut().submit_password(Some("wrong".into()));
            finish_tool_prepare(&mut harness, &started, &release, &finished);
            let prompt = harness.state().password_prompt.as_ref().expect("wrong-password prompt");
            assert!(prompt.error.is_some());
            assert_eq!(prompt.then, Some("export.text"));
            assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());
            harness.state_mut().submit_password(Some("secret".into()));
            finish_tool_prepare(&mut harness, &started, &release, &finished);
            assert!(harness.state().password_prompt.is_none() && harness.state().pending_opens.is_empty());
            assert_eq!(harness.state().views.len(), 1);
            assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
            let doc = harness.state().session.get(harness.state().views[0].id).expect("unlocked document");
            assert_eq!(doc.path.as_deref(), Some(file.picked_path().as_str()));
            assert!(!doc.dirty && doc.can_undo().is_none());
        }
        assert_eq!(std::fs::read(&file.path).expect("read original protected PDF"), original.as_slice());
    }
}

#[test]
#[allow(clippy::expect_used)]
fn tool_runs_after_async_original_image_and_text_conversion() {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("original PNG header");
        writer.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]).expect("original pixels");
    }
    for (case, name, bytes, pdf_name) in [
        ("image", "original.png", png.as_slice(), "original.pdf"),
        ("text", "original.txt", b"Original deferred-tool text".as_slice(), "original.pdf"),
    ] {
        let file = ToolFile::new(case, name, bytes);
        let mut harness = tool_harness(vec![file.picked_path()], false);
        let (gate, started, release, finished) = open_worker::prepare_gate();
        harness.state_mut().open_prepare_gate = Some(gate);
        harness.state_mut().run_command("export.text");
        harness.step();
        finish_tool_prepare(&mut harness, &started, &release, &finished);
        assert!(harness.state().pending_opens.is_empty());
        assert_eq!(harness.state().views.len(), 1);
        assert_eq!(harness.state().active, Some(0));
        let doc = harness.state().session.get(harness.state().views[0].id).expect("converted PDF");
        assert_eq!(doc.name, pdf_name);
        assert!(doc.path.is_none() && doc.dirty, "conversion remains a new unsaved PDF");
        assert_eq!(doc.info.pages.len(), 1);
        assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))), "tool runs on the converted PDF");
        assert_eq!(std::fs::read(&file.path).expect("read original conversion input"), bytes);
    }
}

#[test]
#[allow(clippy::expect_used)]
fn tool_preserves_inline_open_before_first_frame_and_in_live_inline_mode() {
    let original = fixture(1);
    for live in [false, true] {
        let file = ToolFile::new(if live { "live-inline" } else { "startup-inline" }, "inline.pdf", &original);
        if live {
            let mut harness = tool_harness(vec![file.picked_path()], true);
            harness.state_mut().run_command("export.text");
            harness.step();
            assert!(harness.state().pending_opens.is_empty());
            assert_eq!(harness.state().views.len(), 1);
            assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
        } else {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").expect("language preference");
            app.pick_override = Some(vec![file.picked_path()]);
            app.run_command("export.text");
            app.process_picked();
            assert!(app.ctx.is_none() && app.pending_opens.is_empty());
            assert_eq!(app.views.len(), 1);
            assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))));
        }
        assert_eq!(std::fs::read(&file.path).expect("read original inline PDF"), original);
    }
}

#[test]
#[allow(clippy::expect_used)]
fn tool_does_not_run_when_async_read_fails() {
    let file = ToolFile::new("read-failure", "original.pdf", &fixture(1));
    let other = ToolFile::new("read-failure-b", "ordinary-b.pdf", &fixture(1));
    let missing = file.directory.join("missing.pdf").to_string_lossy().into_owned();
    let mut harness = tool_harness(vec![missing], false);
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    harness.state_mut().run_command("export.text");
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("missing file reached read barrier");
    harness.state_mut().open_path(&other.picked_path());
    release.send(()).expect("release missing-file read");
    finished.recv_timeout(Duration::from_secs(5)).expect("missing-file read completed");
    started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
    release.send(()).expect("release ordinary B");
    finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is ready");
    harness.step();
    assert!(harness.state().pending_opens.is_empty() && harness.state().password_prompt.is_none());
    assert_eq!(harness.state().views.len(), 1, "read failure does not block another ready open");
    assert!(harness.state().dialog.is_none());
    let doc = harness.state().session.get(harness.state().views[0].id).expect("ordinary B after failure");
    assert_eq!(doc.path.as_deref(), Some(other.picked_path().as_str()));
    assert!(harness.state().toast.as_ref().is_some_and(|toast| toast.0.starts_with("Couldn't read")));
    harness.state_mut().open_bytes("later.pdf", None, fixture(1)).expect("open unrelated later document");
    assert!(harness.state().dialog.is_none(), "failed continuation cannot run on a later file");
}

#[test]
#[allow(clippy::expect_used)]
fn tool_inline_password_continuation_waits_for_unlock() {
    let original = protected_tool_fixture();
    let file = ToolFile::new("inline-password", "protected.pdf", &original);
    let mut harness = tool_harness(vec![file.picked_path()], true);
    harness.state_mut().run_command("export.text");
    harness.step();
    assert!(harness.state().pending_opens.is_empty() && harness.state().views.is_empty() && harness.state().dialog.is_none());
    assert_eq!(harness.state().password_prompt.as_ref().expect("inline password prompt").then, Some("export.text"));
    harness.state_mut().submit_password(Some("wrong".into()));
    let prompt = harness.state().password_prompt.as_ref().expect("inline wrong-password prompt");
    assert!(prompt.error.is_some());
    assert_eq!(prompt.then, Some("export.text"));
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());
    harness.state_mut().submit_password(Some("secret".into()));
    assert!(harness.state().pending_opens.is_empty() && harness.state().password_prompt.is_none());
    assert_eq!(harness.state().views.len(), 1);
    assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
    assert_eq!(std::fs::read(&file.path).expect("read original inline protected PDF"), original.as_slice());
}

#[test]
#[allow(clippy::expect_used)]
fn tool_modal_keeps_picked_recipient_when_other_open_is_ready() {
    for same_batch in [true, false] {
        let timing = if same_batch { "batch" } else { "later" };
        let picked = ToolFile::new(&format!("recipient-{timing}-a"), "picked-a.pdf", &fixture(2));
        let other = ToolFile::new(&format!("recipient-{timing}-b"), "ordinary-b.pdf", &fixture(1));
        let mut harness = tool_harness(vec![picked.picked_path()], false);
        let (gate, started, release, finished) = open_worker::prepare_gate();
        harness.state_mut().open_prepare_gate = Some(gate);
        harness.state_mut().run_command("export.text");
        harness.step();
        started.recv_timeout(Duration::from_secs(5)).expect("picked A reached prepare barrier");
        harness.state_mut().open_path(&other.picked_path());
        release.send(()).expect("release picked A");
        finished.recv_timeout(Duration::from_secs(5)).expect("picked A is ready");
        started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
        if same_batch {
            release.send(()).expect("release ordinary B before A admission");
            finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is also ready");
        }
        harness.step();
        assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
        let active = harness.state().active.expect("active Export recipient");
        let doc = harness.state().session.get(harness.state().views[active].id).expect("Export recipient document");
        assert_eq!(doc.path.as_deref(), Some(picked.picked_path().as_str()), "Export must retain picked A in {timing} completion timing");
        assert_eq!(doc.info.pages.len(), 2);
        assert!(!doc.dirty && doc.can_undo().is_none());
        assert_eq!(harness.state().views.len(), 1);
        assert_eq!(harness.state().pending_opens.len(), 1);
        if !same_batch {
            release.send(()).expect("release ordinary B after A Export opens");
            finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is now ready");
        }
        harness.run_steps(2);
        assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
        assert_eq!(harness.state().active, Some(active));
        assert_eq!(harness.state().views.len(), 1, "ready B stays unadmitted while A's modal is open");
        assert_eq!(harness.state().pending_opens.len(), 1);
        assert_eq!(harness.state().pending_opens[0].path.as_deref(), Some(other.picked_path().as_str()));
        // Use the actual modal's Cancel button, rather than clearing its state directly.
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert!(harness.state().dialog.is_none() && harness.state().pending_opens.is_empty());
        assert_eq!(harness.state().views.len(), 2, "cancel releases B's retained result exactly once");
        let active = harness.state().active.expect("ordinary B activates after cancel");
        let doc = harness.state().session.get(harness.state().views[active].id).expect("ordinary B document");
        assert_eq!(doc.path.as_deref(), Some(other.picked_path().as_str()));
        assert_eq!(doc.info.pages.len(), 1);
        harness.run_steps(2);
        assert_eq!(harness.state().views.len(), 2);
        assert!(harness.state().dialog.is_none(), "A's tool does not run again after B admission");
    }
}

#[test]
#[allow(clippy::expect_used)]
fn tool_password_prompt_retains_other_ready_open_through_retry() {
    let protected = protected_tool_fixture();
    let picked = ToolFile::new("password-recipient-a", "protected-a.pdf", &protected);
    let other = ToolFile::new("password-recipient-b", "ordinary-b.pdf", &fixture(2));
    let mut harness = tool_harness(vec![picked.picked_path()], false);
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    harness.state_mut().run_command("export.text");
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("encrypted A reached prepare barrier");
    harness.state_mut().open_path(&other.picked_path());
    release.send(()).expect("release encrypted A");
    finished.recv_timeout(Duration::from_secs(5)).expect("encrypted A is ready");
    started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
    release.send(()).expect("release ordinary B");
    finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is ready before prompt admission");
    harness.step();
    let prompt = harness.state().password_prompt.as_ref().expect("picked A password prompt");
    assert_eq!(prompt.path.as_deref(), Some(picked.picked_path().as_str()));
    assert_eq!(prompt.then, Some("export.text"));
    harness.run_steps(2);
    assert!(harness.state().views.is_empty());
    assert_eq!(harness.state().pending_opens.len(), 1, "B cannot replace A's visible password prompt");
    harness.state_mut().submit_password(Some("wrong".into()));
    finish_tool_prepare(&mut harness, &started, &release, &finished);
    // During the retry gap no modal exists, so ordinary B may admit. A still owns its
    // continuation and its wrong-password prompt, and success must reactivate A.
    assert_eq!(harness.state().views.len(), 1);
    let prompt = harness.state().password_prompt.as_ref().expect("picked A wrong-password prompt");
    assert_eq!(prompt.path.as_deref(), Some(picked.picked_path().as_str()));
    assert!(prompt.error.is_some());
    assert_eq!(prompt.then, Some("export.text"));
    assert!(harness.state().dialog.is_none());
    harness.state_mut().submit_password(Some("secret".into()));
    finish_tool_prepare(&mut harness, &started, &release, &finished);
    assert!(harness.state().password_prompt.is_none() && harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 2);
    assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
    let active = harness.state().active.expect("A reactivates for Export");
    let doc = harness.state().session.get(harness.state().views[active].id).expect("A Export recipient");
    assert_eq!(doc.path.as_deref(), Some(picked.picked_path().as_str()));
    assert_eq!(doc.info.pages.len(), 1);
    assert!(!doc.dirty && doc.can_undo().is_none());
    harness.run_steps(2);
    assert_eq!(harness.state().active, Some(active));
    harness.get_by_label("Cancel").click();
    harness.run_steps(2);
    assert!(harness.state().dialog.is_none() && harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 2);
    assert_eq!(std::fs::read(&picked.path).expect("read original encrypted A"), protected.as_slice());
}
