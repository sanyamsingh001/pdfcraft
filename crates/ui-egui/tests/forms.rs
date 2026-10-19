//! Filling in a form in the real shell (egui_kittest): typing, Tab, check boxes, radios, choices.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::control::{ControlClient, Reply};
use serde_json::{Value, json};

fn attach_control(h: &mut Harness<'static, PdfCraftApp>) -> ControlClient {
    let ctx = h.ctx.clone();
    h.state_mut().attach_control(&ctx)
}

/// Exercise the public control transport and advance only deterministic harness frames.
fn control_call(h: &mut Harness<'static, PdfCraftApp>, client: &ControlClient, method: &str, params: Value) -> Reply {
    let rx = client.send(method, params);
    for _ in 0..30 {
        h.step();
        if let Ok(reply) = rx.try_recv() {
            return reply;
        }
    }
    panic!("{method}: no reply after 30 frames");
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        // The whole 300×400 pt page on screen.
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

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// Click the centre of a field's widget.
fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str, widget: usize) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, widget).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

#[test]
fn typing_into_a_text_field_and_tabbing_on() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    assert!(h.state().views[0].forms.focus.is_some(), "the editor opened");
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "name"), ["Ada Lovelace"]);
    assert_eq!(
        h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()),
        Some("country"),
        "Tab moves to the next field that takes typing or a choice"
    );
    // The country list is open; Escape closes it. Escape also abandons a draft.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    click_field(&mut h, "city", 0);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(value(&h, "city").is_empty());
    assert!(h.state().views[0].forms.focus.is_none());
}

#[test]
fn check_boxes_radios_and_choices() {
    let mut h = harness();
    click_field(&mut h, "agree", 0);
    assert_eq!(value(&h, "agree"), ["Yes"]);
    click_field(&mut h, "agree", 0);
    assert!(value(&h, "agree").is_empty());
    click_field(&mut h, "size", 1);
    assert_eq!(value(&h, "size"), ["L"]);
    click_field(&mut h, "size", 0);
    assert_eq!(value(&h, "size"), ["S"]);
    click_field(&mut h, "country", 0);
    egui_kittest::kittest::Queryable::get_by_label(&h, "France").click();
    h.run_steps(4);
    assert_eq!(value(&h, "country"), ["fr"]);
    // Clear form takes everything back; undo restores it.
    assert!(h.state_mut().execute("form.clear"));
    h.run_steps(2);
    assert!(value(&h, "country").is_empty() && value(&h, "size").is_empty());
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(value(&h, "country"), ["fr"]);
}

#[test]
fn tabbing_into_a_filled_field_selects_it_so_typing_replaces() {
    let mut h = harness();
    click_field(&mut h, "country", 0);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Canada").click();
    h.run_steps(3);
    click_field(&mut h, "city", 0);
    h.event(egui::Event::Text("Paris".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(value(&h, "city"), ["Paris"]);
    // name → (Tab) country → (Tab) city, whose text is selected on entry.
    click_field(&mut h, "name", 0);
    h.key_press(egui::Key::Tab);
    h.run_steps(3);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.as_str()), Some("city"));
    h.event(egui::Event::Text("Lyon".into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(value(&h, "city"), ["Lyon"]);
}

#[test]
fn date_fields_offer_a_calendar() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::AddField {
        page: 0,
        rect: [50.0, 40.0, 200.0, 60.0],
        kind: pdfcraft_engine::NewField::Date,
        name: Some("due".into()),
    });
    h.run_steps(4);
    click_field(&mut h, "due", 0);
    let (y, m, _) = h.state().session.today();
    let month =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][(m - 1) as usize];
    h.get_by_label(&format!("{month} {y}"));
    h.get_by_label("›").click();
    h.run_steps(2);
    h.get_by_label("15").click();
    h.run_steps(4);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    assert_eq!(value(&h, "due"), vec![format!("{nm:02}/15/{ny}")], "picked in the field's format");
}

fn save_path(test: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pdfcraft-form-{test}-{}.pdf", std::process::id()))
}

/// Check the file actually written, including its appearance, through a fresh document.
fn saved_field(path: &std::path::Path, name: &str, expected: &str) {
    use std::sync::Arc;
    let bytes = std::fs::read(path).unwrap();
    let mut reopened = PdfCraftApp::new();
    reopened.open_bytes("saved.pdf", None, bytes.clone()).unwrap();
    let id = reopened.views[0].id;
    let field = reopened.session.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap();
    assert_eq!(field.value, [expected]);
    let cos = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    let widget = cos.get(field.widgets[0].obj).as_dict().cloned().unwrap();
    let normal = widget.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").expect("normal appearance");
    let pdfcraft_cos::Object::Stream(stream) = &*cos.get(normal) else { panic!("normal appearance is not a stream") };
    let appearance = String::from_utf8(stream.decoded().unwrap()).unwrap();
    assert!(appearance.contains(expected), "the saved appearance must display the value: {appearance}");
}

#[test]
fn focused_form_text_is_committed_before_save_and_save_as_shortcuts() {
    for (name, modifiers) in [("save", egui::Modifiers::COMMAND), ("save-as", egui::Modifiers::COMMAND | egui::Modifiers::SHIFT)] {
        let path = save_path(name);
        let mut h = harness();
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        click_field(&mut h, "name", 0);
        h.event(egui::Event::Text("Browser Form 123".into()));
        h.run_steps(2);
        assert!(value(&h, "name").is_empty(), "typing is still a draft");
        assert!(h.state().views[0].forms.focus.is_some());
        h.key_press_modifiers(modifiers, egui::Key::S);
        h.run_steps(3);
        saved_field(&path, "name", "Browser Form 123");
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn saving_an_inactive_tab_flushes_only_that_tabs_form_draft() {
    let path = save_path("inactive");
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Original Tab".into()));
    h.run_steps(2);
    let app = h.state_mut();
    app.open_bytes("other.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
    app.save_override = Some(path.to_string_lossy().into_owned());
    assert!(app.save_view(0, pdfcraft_ui_egui::SaveTarget::InPlace));
    assert_eq!(app.active, Some(1));
    let other = app.session.get(app.views[1].id).unwrap();
    assert!(other.form.iter().find(|f| f.name == "name").unwrap().value.is_empty());
    assert!(!other.dirty);
    saved_field(&path, "name", "Original Tab");
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_focused_form_text_blocks_save_and_preserves_the_draft() {
    let path = save_path("invalid");
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "name".into(),
        props: Box::new(pdfcraft_engine::FieldProps {
            validate: Some(pdfcraft_engine::form_scripts::Validate::Range { min: Some(0.0), max: Some(10.0) }),
            ..Default::default()
        }),
    }));
    h.run_steps(2);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("99".into()));
    h.run_steps(2);
    std::fs::write(&path, b"existing destination").unwrap();
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(!h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    assert_eq!(std::fs::read(&path).unwrap(), b"existing destination");
    assert!(value(&h, "name").is_empty());
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "99");
    assert!(h.state().toast.as_ref().unwrap().0.contains("failed"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn save_applies_an_already_queued_form_edit_before_serializing() {
    let path = save_path("queued");
    let mut h = harness();
    h.state_mut().views[0].pending_edit =
        Some(pdfcraft_engine::Edit::SetFieldValue { name: "name".into(), value: pdfcraft_engine::FieldValue::Text("Queued Form".into()) });
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    saved_field(&path, "name", "Queued Form");
    assert!(h.state().views[0].pending_edit.is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn merely_focusing_a_form_does_not_require_a_close_prompt() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    assert_eq!(h.state().first_dirty(), None);
    h.state_mut().request_close_tab(0);
    assert!(h.state().views.is_empty());
    assert!(h.state().close_request.is_none());
}

#[test]
fn a_focused_form_draft_offers_save_discard_and_cancel_before_closing() {
    use egui_kittest::kittest::Queryable;
    for choice in ["Save", "Don't save", "Cancel"] {
        let path = save_path(choice);
        let mut h = harness();
        click_field(&mut h, "name", 0);
        h.event(egui::Event::Text("Unsaved Draft".into()));
        h.run_steps(2);
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
        assert_eq!(h.state().first_dirty(), Some(0));
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        h.state_mut().request_close_tab(0);
        assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(h.state().views[0].id)));
        h.run_steps(2);
        h.get_by_label(choice).click();
        h.run_steps(4);
        match choice {
            "Save" => {
                assert!(h.state().views.is_empty());
                saved_field(&path, "name", "Unsaved Draft");
            }
            "Don't save" => assert!(h.state().views.is_empty()),
            _ => {
                assert_eq!(h.state().views.len(), 1);
                assert_eq!(h.state().first_dirty(), Some(0));
            }
        }
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn close_all_preserves_focused_form_drafts_until_the_user_answers() {
    let mut h = harness();
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Keep Me".into()));
    h.run_steps(2);
    h.state_mut().open_bytes("clean.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
    h.state_mut().close_all();
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::All));
    assert_eq!(h.state().first_dirty(), Some(0));
}

#[test]
fn rejected_tab_blur_and_mouse_save_keep_input_editable_across_frames() {
    use egui_kittest::kittest::Queryable;
    for trigger in ["tab", "blur", "checkbox-blur", "mouse-save"] {
        let path = save_path(trigger);
        let mut h = harness();
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
            name: "name".into(),
            props: Box::new(pdfcraft_engine::FieldProps {
                validate: Some(pdfcraft_engine::form_scripts::Validate::Range { min: Some(0.0), max: Some(10.0) }),
                ..Default::default()
            }),
        }));
        h.run_steps(2);
        click_field(&mut h, "name", 0);
        h.event(egui::Event::Text("99".into()));
        h.run_steps(2);
        std::fs::write(&path, b"old destination").unwrap();
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        match trigger {
            "tab" => h.key_press(egui::Key::Tab),
            "blur" => click_field(&mut h, "city", 0),
            "checkbox-blur" => click_field(&mut h, "agree", 0),
            _ => h.get_by_label("Save (M4)").click(),
        }
        h.run_steps(8);
        let focus = h.state().views[0].forms.focus.as_ref().expect("rejected field stays editable");
        assert_eq!(focus.name, "name");
        assert_eq!(focus.text, "99");
        assert!(!h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
        h.run_steps(6);
        assert_eq!(std::fs::read(&path).unwrap(), b"old destination");
        assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "99");
        // Correct the recovered editor through real keyboard input, then Save succeeds.
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.event(egui::Event::Text("5".into()));
        h.run_steps(2);
        assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
        saved_field(&path, "name", "5");
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_rejected_form_draft_can_be_explicitly_discarded_with_escape() {
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "name".into(),
        props: Box::new(pdfcraft_engine::FieldProps { read_only: Some(true), ..Default::default() }),
    }));
    h.state_mut().views[0].pending_edit =
        Some(pdfcraft_engine::Edit::SetFieldValue { name: "name".into(), value: pdfcraft_engine::FieldValue::Text("Rejected".into()) });
    h.run_steps(5);
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "Rejected");
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(h.state().views[0].forms.focus.is_none());
    assert!(h.state().views[0].pending_edit.is_none());
    assert!(value(&h, "name").is_empty());
}

#[test]
fn an_inactive_tabs_validation_print_request_prints_that_tab() {
    let saved = save_path("script-source");
    let printed = save_path("script-print");
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldScript {
        name: "name".into(),
        event: "validate".into(),
        script: Some("this.print();".into()),
    }));
    h.run_steps(3);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Source Document".into()));
    h.run_steps(2);
    let source = h.state().views[0].id;
    let bytes = h.state().session.create_blank(200.0, 300.0, 2).unwrap();
    h.state_mut().open_bytes("other.pdf", None, bytes.as_ref().clone()).unwrap();
    h.state_mut().save_override = Some(saved.to_string_lossy().into_owned());
    assert!(h.state_mut().save_view(0, pdfcraft_ui_egui::SaveTarget::InPlace));
    assert_eq!(h.state().active, Some(1));
    assert_eq!(h.state().print_draft.document, Some(source));
    h.run_steps(3);
    h.state_mut().print_draft.printer = None;
    h.state_mut().save_override = Some(printed.to_string_lossy().into_owned());
    assert!(h.state_mut().print_now());
    let cos = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(&printed).unwrap())).unwrap();
    assert_eq!(pdfcraft_model::pages(&cos).len(), 1, "prints the original one-page document, not the active two-page tab");
    let _ = std::fs::remove_file(saved);
    let _ = std::fs::remove_file(printed);
}

#[test]
fn form_draft_tab_marker_and_control_dirty_match_close_detection() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness();
    let client = attach_control(&mut h);
    let name = h.state().session.get(h.state().views[0].id).unwrap().display_name();
    click_field(&mut h, "name", 0);
    assert_eq!(control_call(&mut h, &client, "ui.state", json!({})).unwrap()["documents"][0]["dirty"], false, "focus alone is clean");
    h.get_by_label(&name);
    h.event(egui::Event::Text("Unsaved Draft".into()));
    h.run_steps(3);
    assert_eq!(control_call(&mut h, &client, "ui.state", json!({})).unwrap()["documents"][0]["dirty"], true);
    assert_eq!(h.state().first_dirty(), Some(0));
    h.get_by_label(&format!("{name} (edited)"));
    let commands = control_call(&mut h, &client, "ui.commands", json!({})).unwrap();
    for id in ["file.save", "file.save_as"] {
        let command = commands["commands"].as_array().unwrap().iter().find(|command| command["id"] == id).unwrap();
        assert_eq!(command["enabled"], true, "drafts can be saved without requiring blur");
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(
        control_call(&mut h, &client, "ui.state", json!({})).unwrap()["documents"][0]["dirty"],
        false,
        "discarding the draft restores clean state"
    );
    assert_eq!(h.state().first_dirty(), None);
    h.get_by_label(&name);
}

#[test]
fn control_save_rejection_keeps_the_editor_usable_for_correction() {
    let path = save_path("control-rejection");
    let mut h = harness();
    let client = attach_control(&mut h);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "name".into(),
        props: Box::new(pdfcraft_engine::FieldProps {
            validate: Some(pdfcraft_engine::form_scripts::Validate::Range { min: Some(0.0), max: Some(10.0) }),
            ..Default::default()
        }),
    }));
    h.run_steps(2);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("99".into()));
    h.run_steps(2);
    std::fs::write(&path, b"existing file").unwrap();
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(control_call(&mut h, &client, "ui.command", json!({ "id": "file.save" })).is_err(), "rejected drafts are not reported complete");
    h.run_steps(5);
    assert_eq!(std::fs::read(&path).unwrap(), b"existing file");
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "99");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("5".into()));
    h.run_steps(2);
    assert_eq!(control_call(&mut h, &client, "ui.command", json!({ "id": "file.save" })).unwrap()["status"], "completed");
    saved_field(&path, "name", "5");
    assert_eq!(control_call(&mut h, &client, "ui.state", json!({})).unwrap()["documents"][0]["dirty"], false);
    let _ = std::fs::remove_file(path);
}

/// Runtime-only synthetic AcroForm: one logical text field, optionally on two widgets.
fn one_field_pdf(two_widgets: bool) -> Vec<u8> {
    let widgets = if two_widgets { "7 0 R 8 0 R" } else { "7 0 R" };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [{widgets}] >>"),
        "<< /Length 0 >>\nstream\n\nendstream".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Fields [9 0 R] /DR << /Font << /F1 5 0 R >> >> /DA (/F1 12 Tf 0 g) >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /Parent 9 0 R /P 3 0 R /Rect [20 70 180 100] /F 4 >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /Parent 9 0 R /P 3 0 R /Rect [20 120 180 150] /F 4 >>".to_string(),
        format!("<< /FT /Tx /T (name) /V () /Kids [{widgets}] >>"),
    ];
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

fn one_field_harness(two_widgets: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("one-field.pdf", None, one_field_pdf(two_widgets)).unwrap();
        app.set_option("left", "closed").unwrap();
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

#[test]
fn tabbing_to_the_same_logical_field_never_saves_its_stale_opening_value() {
    for two_widgets in [false, true] {
        let path = save_path(&format!("tab-wrap-{two_widgets}"));
        let mut h = one_field_harness(two_widgets);
        click_field(&mut h, "name", 0);
        h.event(egui::Event::Text("Ada".into()));
        h.run_steps(2);
        h.key_press(egui::Key::Tab);
        h.run_steps(4);
        assert_eq!(value(&h, "name"), ["Ada"]);
        let focus = h.state().views[0].forms.focus.as_ref().unwrap();
        assert_eq!(focus.name, "name");
        assert_eq!(focus.widget, if two_widgets { 1 } else { 0 });
        assert_eq!(focus.text, "Ada", "the untouched editor follows the committed value");
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
        h.run_steps(3);
        saved_field(&path, "name", "Ada");
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn tabbing_into_a_calculated_field_uses_its_new_value_and_preserves_it_on_save() {
    let path = save_path("calculated-tab");
    let mut h = harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps {
        name: "country".into(),
        props: Box::new(pdfcraft_engine::FieldProps { read_only: Some(true), ..Default::default() }),
    }));
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldScript {
        name: "city".into(),
        event: "calculate".into(),
        script: Some("event.value = getField('name').value + ' computed';".into()),
    }));
    h.run_steps(3);
    click_field(&mut h, "name", 0);
    h.event(egui::Event::Text("Ada".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h, "city"), ["Ada computed"]);
    let focus = h.state().views[0].forms.focus.as_ref().unwrap();
    assert_eq!(focus.name, "city");
    assert_eq!(focus.text, "Ada computed");
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    saved_field(&path, "name", "Ada");
    saved_field(&path, "city", "Ada computed");
    let _ = std::fs::remove_file(path);
}

#[test]
fn script_updates_refresh_pristine_editors_and_preserve_actual_user_drafts() {
    let mut h = harness();
    click_field(&mut h, "city", 0);
    h.state_mut().js_console.input = "getField('city').value = 'Script update';".into();
    h.state_mut().run_console();
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "Script update");
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().original, ["Script update"]);
    h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("User draft".into()));
    h.run_steps(2);
    h.state_mut().js_console.input = "getField('city').value = 'Later script';".into();
    h.state_mut().run_console();
    h.run_steps(3);
    assert_eq!(value(&h, "city"), ["Later script"]);
    assert_eq!(h.state().views[0].forms.focus.as_ref().unwrap().text, "User draft", "a model refresh never replaces typed input");
    let path = save_path("script-user-draft");
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    saved_field(&path, "city", "User draft");
    let _ = std::fs::remove_file(path);
}
