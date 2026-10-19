//! Push buttons run their actions (Reset form, Named, …) without a JavaScript engine.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Two pages; page 1 has a text field "name" (filled), a Reset button, a Next page button and a
/// Print button (a JavaScript one-liner).
fn fixture() -> Vec<u8> {
    reset_list_fixture(None, false)
}

fn reset_list_fixture(fields: Option<&str>, exclude: bool) -> Vec<u8> {
    reset_list_fixture_with_offscreen_button(fields, exclude, false)
}

fn reset_list_fixture_with_offscreen_button(fields: Option<&str>, exclude: bool, offscreen: bool) -> Vec<u8> {
    let (pages, first_page, second_page, text, reset_page, reset_rect) = if offscreen {
        (
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 800] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 7 0 R 8 0 R 9 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [20 740 280 760] /P 3 0 R >>",
            4,
            "20 680 100 700",
        )
    } else {
        (
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [20 340 280 360] /P 3 0 R >>",
            3,
            "20 300 100 320",
        )
    };
    let fields = fields.map(|value| format!("/Fields {value}")).unwrap_or_default();
    let reset = format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [{reset_rect}] /P {reset_page} 0 R /A << /S /ResetForm {fields} /Flags {} >> >>",
        i32::from(exclude)
    );
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        pages,
        first_page,
        second_page,
        text,
        &reset,
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (next) /Rect [120 300 200 320] /P 3 0 R /A << /S /Named /N /NextPage >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (print) /Rect [220 300 280 320] /P 3 0 R /AA << /U << /S /JavaScript /JS (this.print\\({bUI: true}\\);) >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (save) /Rect [20 260 100 280] /P 3 0 R /A << /S /Named /N /Save >> >>",
        "[]", // 10: a valid indirect empty field-selection array
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

#[test]
fn buttons_reset_navigate_and_print() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-buttons-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("buttons.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let path_str = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("buttons.pdf", Some(path_str), fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "reset");
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.iter().find(|f| f.name == "name").unwrap().value.is_empty(), "Reset form cleared the field");
    }
    click_field(&mut h, "print");
    assert_eq!(h.state().dialog, Some(Dialog::Print), "this.print() opens the Print dialog");
    h.state_mut().dialog = None;
    h.run_steps(2);
    // A document's Save named action never writes the file unasked (PdfCraft's XFA buttons
    // use SaveAs, which opens the Save As dialog, as a script's `execMenuItem` does): the
    // reset above made the document dirty, and the click leaves it so.
    let before = std::fs::read(&path).unwrap();
    click_field(&mut h, "save");
    h.run_steps(4);
    h.get_by_label_contains("isn't supported");
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty, "not saved");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the file is untouched");
    click_field(&mut h, "next");
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1, "the NextPage action");
}

#[test]
fn explicit_empty_reset_buttons_preserve_values_bytes_and_history() {
    for fields in [None, Some("[]"), Some("10 0 R")] {
        for exclude in [false, true] {
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("reset-list.pdf", None, reset_list_fixture(fields, exclude)).unwrap();
                app.set_option("zoom", "150").unwrap();
                app
            });
            h.run_steps(6);
            let id = h.state().views[0].id;
            let before = h.state().session.get(id).unwrap();
            assert!(!before.dirty, "the fixture starts clean");
            let history = (before.can_undo().map(str::to_owned), before.can_redo().map(str::to_owned));
            let original = h.state().session.save_bytes(id).unwrap();
            click_field(&mut h, "reset");
            let after = h.state().session.get(id).unwrap();
            let no_fields = fields.is_some() && !exclude;
            let expected = if no_fields { vec!["Ada".to_string()] } else { Vec::new() };
            assert_eq!(after.form.iter().find(|f| f.name == "name").unwrap().value, expected, "Fields {fields:?}, exclude {exclude}");
            let saved = h.state().session.save_bytes(id).unwrap();
            if no_fields {
                assert!(!after.dirty, "an empty inclusion does not dirty the document");
                assert_eq!((after.can_undo().map(str::to_owned), after.can_redo().map(str::to_owned)), history);
                assert_eq!(saved, original, "values, appearances and the rest of the PDF are unchanged");
            } else {
                assert!(after.dirty, "reset-all changes the filled field");
                assert!(after.can_undo().is_some());
            }
            let mut reopened = pdfcraft_engine::Session::new();
            let reopened_id = reopened.open("reopened.pdf", None, saved, None).unwrap();
            assert_eq!(reopened.get(reopened_id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value, expected);
        }
    }
}

#[test]
fn explicit_empty_reset_preserves_an_offscreen_typing_draft() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-empty-reset-draft-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("saved.pdf");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("offscreen-reset.pdf", None, reset_list_fixture_with_offscreen_button(Some("[]"), false, true)).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("layout", "continuous").unwrap();
        app.set_option("animation_effects", "off").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    click_field(&mut h, "name");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);
    let id = h.state().views[0].id;
    let draft = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].forms.focus.as_ref().map(|f| (f.name.clone(), f.text.clone()));
    let value = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value.clone();
    assert_eq!(draft(&h), Some(("name".into(), "Ada Lovelace".into())), "real typing creates the draft");
    assert_eq!(value(&h), ["Ada"], "the draft has not been committed");
    // Hover the canvas gutter without a click, then wheel until the editor's page is
    // completely off-screen. Tall pages keep the second-page button visible at that point.
    let viewport = h.state().views[0].viewport_rect();
    h.hover_at(egui::pos2(viewport.left() + 4.0, viewport.center().y));
    h.run_steps(1);
    for _ in 0..20 {
        let ready = {
            let s = h.state();
            let view = &s.views[0];
            let doc = s.session.get(id).unwrap();
            let button = doc.form.iter().find(|f| f.name == "reset").unwrap();
            view.page_screen_rect(0).is_none()
                && pdfcraft_ui_egui::forms_ui::field_screen_rect(view, &doc.info, button, 0)
                    .is_some_and(|rect| view.viewport_rect().contains(rect.min) && view.viewport_rect().contains(rect.max))
        };
        if ready {
            break;
        }
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -150.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
    }
    assert!(h.state().views[0].page_screen_rect(0).is_none(), "the draft's page must be fully off-screen");
    let reset_rect = {
        let s = h.state();
        let doc = s.session.get(id).unwrap();
        let button = doc.form.iter().find(|f| f.name == "reset").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, button, 0).expect("the reset button is on page two")
    };
    assert!(h.state().views[0].viewport_rect().contains(reset_rect.min) && h.state().views[0].viewport_rect().contains(reset_rect.max));
    assert_eq!(draft(&h), Some(("name".into(), "Ada Lovelace".into())), "scrolling left an active draft");
    assert_eq!(value(&h), ["Ada"], "the button click must encounter uncommitted typing");
    click_field(&mut h, "reset");
    assert_eq!(draft(&h), Some(("name".into(), "Ada Lovelace".into())), "resetting no fields preserves the off-screen draft");
    assert_eq!(value(&h), ["Ada"]);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    h.run_steps(3);
    assert_eq!(value(&h), ["Ada Lovelace"], "Save commits the preserved draft from page two");
    let mut reopened = pdfcraft_engine::Session::new();
    let reopened_id = reopened.open("saved.pdf", None, std::fs::read(&out).unwrap().into(), None).unwrap();
    assert_eq!(reopened.get(reopened_id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value, ["Ada Lovelace"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// One page: a text field "name" and two push buttons whose Hide actions hide it and show it again.
fn show_hide_fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /F 4 /Rect [20 340 280 360] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hide) /Rect [20 300 100 320] /P 3 0 R /A << /S /Hide /T (name) >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (show) /Rect [120 300 200 320] /P 3 0 R /A << /S /Hide /T 4 0 R /H false >> >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Whether the "name" widget (object 4) has the Hidden annotation flag in the saved document.
fn name_hidden(h: &Harness<'static, PdfCraftApp>) -> bool {
    let s = h.state();
    let bytes = s.session.save_bytes(s.views[0].id).unwrap();
    let doc = pdfcraft_cos::Document::open(bytes).unwrap();
    let f = doc.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().and_then(|d| d.int(b"F")).unwrap_or(0);
    f & 2 != 0
}

#[test]
fn hide_actions_hide_and_show_fields() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("show-hide.pdf", None, show_hide_fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    assert!(!name_hidden(&h));
    // A push button with an action shows the pointing hand, not the "not allowed" cursor.
    let hide_at = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == "hide").unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(hide_at);
    h.run_steps(2);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    click_field(&mut h, "hide");
    assert!(name_hidden(&h), "the Hide action hid the field");
    // A hidden field takes no clicks: clicking where it was doesn't start editing it.
    click_field(&mut h, "name");
    assert!(h.state().views[0].forms.focus.is_none(), "a hidden field can't be focused");
    click_field(&mut h, "show");
    assert!(!name_hidden(&h), "the Hide action with /H false showed it again");
}
