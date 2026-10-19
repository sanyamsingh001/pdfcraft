//! Startup URL completions are delivered without timing or network dependencies.
use pdfcraft_ui_egui::{Dialog, Mode, PasswordPrompt, PdfCraftApp, PropsTab, QuickTool, RightPanel, StartupFile};

const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /PageMode /UseOutlines >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn complete(app: &mut PdfCraftApp, result: Result<Vec<u8>, String>) {
    app.startup_inbox.lock().unwrap().push(StartupFile { name: "startup.pdf".into(), result });
    app.process_startup_files();
}

#[test]
fn startup_without_later_intent_opens_foreground() {
    let mut app = PdfCraftApp::new();
    complete(&mut app, Ok(PDF.to_vec()));
    assert_eq!(app.active, Some(0));
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.right, Some(RightPanel::Bookmarks));
    assert!(app.toast.is_none());
}

#[test]
fn late_startup_preserves_active_edits_navigation_and_workspace() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("work.pdf", None, PDF.to_vec()).unwrap();
    let id = app.views[0].id;
    app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    app.views[0].go_to_page(1);
    app.views[0].set_zoom(0.75);
    app.mode = Mode::Edit;
    app.right = Some(RightPanel::Comments);
    app.quick_tool = QuickTool::Hand;
    app.dialog = Some(Dialog::Properties(PropsTab::Description));
    app.password_prompt = Some(PasswordPrompt {
        name: "another-encrypted.pdf".into(),
        path: None,
        bytes: std::sync::Arc::new(vec![1]),
        input: "draft password".into(),
        error: None,
    });
    let bytes = app.session.get(id).unwrap().bytes.clone();
    let goto = app.views[0].goto;
    complete(&mut app, Ok(PDF.to_vec()));
    assert_eq!(app.views.len(), 2, "startup PDF remains available in its own tab");
    assert_eq!(app.active_ids(), Some((0, id)));
    let doc = app.session.get(id).unwrap();
    assert!(doc.dirty);
    assert_eq!(doc.bytes, bytes);
    assert_eq!(app.views[0].current, 1);
    assert_eq!(app.views[0].goto, goto);
    assert_eq!(app.views[0].zoom, 0.75);
    assert_eq!(app.mode, Mode::Edit);
    assert_eq!(app.right, Some(RightPanel::Comments));
    assert_eq!(app.quick_tool, QuickTool::Hand);
    assert_eq!(app.dialog, Some(Dialog::Properties(PropsTab::Description)));
    assert_eq!(app.password_prompt.as_ref().unwrap().input, "draft password");
}

#[test]
fn returning_home_does_not_restore_startup_activation() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("work.pdf", None, PDF.to_vec()).unwrap();
    app.close_tab(0);
    complete(&mut app, Ok(PDF.to_vec()));
    assert_eq!(app.active, None);
    assert_eq!(app.views.len(), 1);
}

#[test]
fn sidebar_choice_on_home_survives_late_startup() {
    let mut app = PdfCraftApp::new();
    app.set_option("panel", "comments").unwrap();
    complete(&mut app, Ok(PDF.to_vec()));
    assert_eq!(app.active, None);
    assert_eq!(app.right, Some(RightPanel::Comments));
}

#[test]
fn failure_is_actionable_and_preserves_newer_error_and_document() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("work.pdf", None, PDF.to_vec()).unwrap();
    let id = app.active_ids();
    app.notify("Newer save failed: disk full");
    complete(&mut app, Err("HTTP 404".into()));
    assert_eq!(app.toast.as_ref().unwrap().0, "Newer save failed: disk full");
    assert_eq!(app.active_ids(), id);
    assert_eq!(app.views.len(), 1);
    app.toast = None; // Foreground notice expired.
    app.process_startup_files();
    let message = &app.toast.as_ref().unwrap().0;
    assert!(message.contains("startup.pdf") && message.contains("HTTP 404"));
    assert!(message.contains("cross-origin") && message.contains("Open"));
    assert_eq!(app.active_ids(), id);
}

#[test]
fn invalid_startup_pdf_reports_error_without_opening_hidden_document() {
    let mut app = PdfCraftApp::new();
    complete(&mut app, Ok(vec![1, 2, 3]));
    assert!(app.views.is_empty());
    assert_eq!(app.active, None);
    assert!(app.toast.as_ref().unwrap().0.contains("Couldn't load startup.pdf"));
}

#[test]
fn fetch_failure_is_rendered_in_the_interface() {
    use egui_kittest::kittest::Queryable;
    let mut h = egui_kittest::Harness::builder().build_eframe(|_| PdfCraftApp::new());
    h.run_steps(3);
    h.state().startup_inbox.lock().unwrap().push(StartupFile { name: "missing.pdf".into(), result: Err("HTTP 403".into()) });
    h.run_steps(2);
    h.get_by_label_contains("Couldn't load missing.pdf: HTTP 403");
}
