//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with_usable_appearance(false)
}

fn harness_with_usable_appearance(usable: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut bytes = include_bytes!("data/form.pdf").to_vec();
        if usable {
            // The imported unchecked widgets share an intentionally blank appearance.
            // Give that success fixture valid Form geometry without adding any ink or
            // changing its field values/state aliases; the shared asset stays unchanged.
            let mut cos = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
            cos.update_dict(pdfcraft_cos::ObjRef::new(7, 0), |d| {
                d.set(b"Type".to_vec(), pdfcraft_cos::Object::name("XObject"));
                d.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("Form"));
                d.set(b"FormType".to_vec(), pdfcraft_cos::Object::Int(1));
                d.set(b"BBox".to_vec(), pdfcraft_cos::Object::Array([0, 0, 16, 16].map(pdfcraft_cos::Object::Int).to_vec()));
            })
            .unwrap();
            let appearance = cos.get(pdfcraft_cos::ObjRef::new(7, 0));
            let pdfcraft_cos::Object::Stream(appearance) = &*appearance else { panic!("blank appearance stream") };
            assert!(appearance.decoded().unwrap().is_empty(), "the geometry repair adds no ink");
            bytes = pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
        }
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, bytes).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().redaction_marks()
}

#[test]
fn marking_applying_and_clearing() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.get_by_label("Redact a PDF");
    // Draw a box in an empty part of the page.
    let (a, b) = (at(&h, 40.0, 280.0), at(&h, 200.0, 320.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(marks(&h), 1);
    // Redact pages ▸ current page.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("Mark Page Range");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Clear all, then undo it.
    h.get_by_label("Clear all").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Redact all asks first; Apply removes the marks (and the page's fields under them).
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactApply));
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!((doc.redaction_marks(), doc.can_undo()), (0, Some("Apply redactions")));
    assert!(doc.form.is_empty(), "the whole page was redacted, fields included");
}

#[test]
fn removing_hidden_information_and_sanitizing() {
    let mut h = harness_with_usable_appearance(true);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldValue {
        name: "name".into(),
        value: pdfcraft_engine::FieldValue::Text("Retained form value".into()),
    }));
    h.run_steps(2);
    h.state_mut().execute("protect.remove_hidden");
    h.run_steps(2);
    h.get_by_label("Remove hidden information");
    {
        let d = &h.state().hidden_draft;
        let fields = d.found.iter().find(|f| f.0 == pdfcraft_engine::Hidden::FormFields).unwrap();
        assert_eq!((fields.1, fields.2), (5, true), "five fields, checked");
    }
    h.get_by_label("Remove").click();
    h.run_steps(3);
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.is_empty());
        assert_eq!(doc.can_undo(), Some("Remove hidden information"));
    }
    assert_flattened_value(h.state(), "Retained form value");
    h.state_mut().undo();
    h.run_steps(2);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.form.len(), 5, "undo restores the form before testing Sanitize itself");
    assert_eq!(doc.form.iter().find(|f| f.name == "name").unwrap().value, ["Retained form value"]);
    h.state_mut().execute("redact.sanitize");
    h.run_steps(2);
    h.get_by_label("Sanitize").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Sanitize document"));
    assert_flattened_value(s, "Retained form value");
}

fn assert_flattened_value(app: &PdfCraftApp, expected: &str) {
    let id = app.views[0].id;
    assert!(app.session.get(id).unwrap().form.is_empty());
    let bytes = app.session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    let id = reopened.open("saved.pdf", None, bytes.clone(), None).unwrap();
    assert!(reopened.get(id).unwrap().form.is_empty(), "the saved document has no interactive fields");
    let mut renderer = pdfcraft_render::PageRenderer::new(bytes, Default::default());
    let page = renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    let text = page.text.unwrap().plain_text();
    assert!(text.contains(expected), "the saved page retains the flattened value: {text}");
}

#[test]
fn incomplete_imported_appearances_refuse_removal_and_sanitization_without_losing_work() {
    for (command, confirm) in [("protect.remove_hidden", "Remove"), ("redact.sanitize", "Sanitize")] {
        // Keep the original incomplete appearance; this is the refusal control.
        let mut h = harness();
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldValue {
            name: "name".into(),
            value: pdfcraft_engine::FieldValue::Text("Retained form value".into()),
        }));
        h.run_steps(2);
        let id = h.state().views[0].id;
        let doc = h.state().session.get(id).unwrap();
        let original_bytes = doc.bytes.clone();
        let fields = doc.form.clone();
        let state = (doc.dirty, doc.edit_generation(), doc.can_undo().map(str::to_string), doc.can_redo().map(str::to_string));
        let saved = h.state().session.save_bytes(id).unwrap();
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        h.get_by_label(confirm).click();
        h.run_steps(3);
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.bytes, original_bytes);
        assert_eq!(doc.form, fields);
        assert_eq!((doc.dirty, doc.edit_generation(), doc.can_undo().map(str::to_string), doc.can_redo().map(str::to_string)), state);
        assert_eq!(h.state().session.save_bytes(id).unwrap(), saved);
        h.get_by_label_contains("normal appearance or placement is unusable");
    }
}
