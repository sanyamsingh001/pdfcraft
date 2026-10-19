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

/// Contributor-original XFA fixture; no network, timing or external document dependency.
fn initialization_script(script: &str) -> Vec<u8> {
    let template = pdfcraft_xfa::fixtures::scripted_template().replace("if (qty.rawValue === null) qty.rawValue = 2;", script);
    pdfcraft_xfa::fixtures::shell(&template)
}

#[test]
fn late_xfa_initialization_delivers_notices_and_errors_without_foreground_hijack() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("work.pdf", None, PDF.to_vec()).unwrap();
    let foreground = app.views[0].id;
    assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    app.views[0].go_to_page(1);
    app.dialog = Some(Dialog::Properties(PropsTab::Description));
    app.notify("Newer save failed: disk full");
    let before = app.session.get(foreground).unwrap().bytes.clone();
    complete(
        &mut app,
        Ok(initialization_script(
            r#"qty.rawValue = 2;
               xfa.host.messageBox("First initialization notice");
               xfa.host.messageBox("Second initialization notice");
               xfa.host.print(); app.execMenuItem("SaveAs");
               throw new Error("Initialization error marker");"#,
        )),
    );
    let source = app.views[1].id;
    assert_eq!(app.active_ids(), Some((0, foreground)));
    assert_eq!(app.views[0].current, 1);
    assert_eq!(app.session.get(foreground).unwrap().bytes, before);
    assert!(app.session.get(foreground).unwrap().dirty);
    assert_eq!(app.dialog, Some(Dialog::Properties(PropsTab::Description)));
    assert_eq!(app.print_draft.document, None, "initialization cannot request printing");
    assert_eq!(app.toast.as_ref().unwrap().0, "Newer save failed: disk full");
    assert!(app.js_console.log.iter().any(|line| line.contains("Initialization error marker")));
    for action in ["print", "save"] {
        assert!(app.js_console.log.iter().any(|line| line.contains(&format!("may only {action}"))), "initialization refusal is visible");
    }
    assert!(app.session.take_js_output(source).is_empty(), "no output is stranded until an edit");
    for message in ["First initialization notice", "Second initialization notice"] {
        app.toast = None;
        app.process_startup_files();
        assert_eq!(app.toast.as_ref().unwrap().0, format!("startup.pdf: {message}"));
        assert_eq!(app.active_ids(), Some((0, foreground)));
    }
    app.toast = None;
    app.process_startup_files();
    assert!(app.toast.is_none(), "notices are consumed exactly once");
}

#[test]
fn late_xfa_notice_is_visible_on_home_without_selecting_its_tab() {
    let mut app = PdfCraftApp::new();
    app.set_option("panel", "comments").unwrap();
    complete(&mut app, Ok(initialization_script(r#"xfa.host.messageBox("Background form ready");"#)));
    assert_eq!(app.active, None);
    assert_eq!(app.right, Some(RightPanel::Comments));
    assert_eq!(app.toast.as_ref().unwrap().0, "startup.pdf: Background form ready");
    let id = app.views[0].id;
    assert!(app.session.take_js_output(id).is_empty());
}

#[test]
fn script_navigation_and_print_keep_the_source_document() {
    use pdfcraft_engine::js::{JsOutput, Request};
    let mut app = PdfCraftApp::new();
    app.open_bytes("source.pdf", None, PDF.to_vec()).unwrap();
    let source = app.views[0].id;
    app.open_bytes("foreground.pdf", None, PDF.to_vec()).unwrap();
    let foreground = app.active_ids();
    app.handle_js(source, JsOutput { requests: vec![Request::GoToPage(1), Request::Print], ..Default::default() });
    assert_eq!(app.active_ids(), foreground);
    assert_eq!(app.views[0].current, 1);
    assert_eq!(app.views[1].current, 0);
    assert_eq!(app.print_draft.document, Some(source));
    assert_eq!(app.print_draft.current_page, 1);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn script_save_as_captures_its_source_and_revalidates_deferred_completion() {
    use pdfcraft_engine::js::{JsOutput, Request};
    let dir = std::path::PathBuf::from(".tmp").join(format!("pdfcraft-startup-script-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for change in ["unchanged", "edited", "closed"] {
        let destination = dir.join(format!("{change}.pdf"));
        let mut h = egui_kittest::Harness::builder().build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.open_bytes("source.pdf", None, PDF.to_vec()).unwrap();
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes("foreground.pdf", None, bytes.as_ref().clone()).unwrap();
            app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
            app
        });
        h.run_steps(2);
        let source = h.state().views[0].id;
        let foreground = h.state().views[1].id;
        let foreground_bytes = h.state().session.get(foreground).unwrap().bytes.clone();
        h.state_mut().pick_override = Some(vec![destination.to_string_lossy().into_owned()]);
        h.state_mut().handle_js(source, JsOutput { requests: vec![Request::SaveAs], ..Default::default() });
        assert!(!destination.exists(), "the chooser completion has not been processed");
        match change {
            "edited" => h.state_mut().session.apply(source, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap(),
            "closed" => h.state_mut().close_tab(0),
            _ => {}
        }
        h.run_steps(2);
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(foreground));
        let doc = h.state().session.get(foreground).unwrap();
        assert_eq!(doc.bytes, foreground_bytes);
        assert!(doc.dirty, "the foreground document was never saved");
        assert!(doc.path.is_none());
        if change == "unchanged" {
            let bytes = std::sync::Arc::new(std::fs::read(&destination).unwrap());
            let doc = pdfcraft_cos::Document::open(bytes).unwrap();
            assert_eq!(pdfcraft_model::pages(&doc).len(), 2, "saved the source, not the one-page foreground tab");
            assert_eq!(h.state().session.get(source).unwrap().path.as_deref(), destination.to_str());
            std::fs::remove_file(&destination).unwrap();
        } else {
            assert!(!destination.exists(), "changed or closed source must not write a stale snapshot");
            assert!(h.state().toast.as_ref().unwrap().0.contains("Couldn't save"));
        }
    }
    std::fs::remove_dir(dir).unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn script_save_as_from_a_closed_document_does_not_fall_back_to_foreground() {
    use pdfcraft_engine::js::{JsOutput, Request};
    let mut app = PdfCraftApp::new();
    app.open_bytes("source.pdf", None, PDF.to_vec()).unwrap();
    let source = app.views[0].id;
    app.open_bytes("foreground.pdf", None, PDF.to_vec()).unwrap();
    app.close_tab(0);
    app.notify("Keep foreground feedback");
    let dir = std::path::PathBuf::from(".tmp").join(format!("pdfcraft-closed-script-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let destination = dir.join("must-not-save.pdf");
    app.save_override = Some(destination.to_string_lossy().into_owned());
    let foreground = app.active_ids();
    app.handle_js(source, JsOutput { requests: vec![Request::SaveAs, Request::Print, Request::GoToPage(1)], ..Default::default() });
    assert_eq!(app.active_ids(), foreground);
    assert_eq!(app.views[0].current, 0);
    assert_eq!(app.print_draft.document, None);
    assert_eq!(app.toast.as_ref().unwrap().0, "Keep foreground feedback");
    assert!(!destination.exists(), "a closed source never saves the foreground document");
    std::fs::remove_dir(dir).unwrap();
}
