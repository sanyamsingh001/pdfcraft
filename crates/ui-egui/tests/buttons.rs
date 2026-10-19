//! Push buttons run their actions (Reset form, Named, …) without a JavaScript engine.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Two pages; page 1 has a text field "name" (filled), a Reset button, a Next page button and a
/// Print button (a JavaScript one-liner).
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [20 340 280 360] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 300 100 320] /P 3 0 R /A << /S /ResetForm >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (next) /Rect [120 300 200 320] /P 3 0 R /A << /S /Named /N /NextPage >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (print) /Rect [220 300 280 320] /P 3 0 R /AA << /U << /S /JavaScript /JS (this.print\\({bUI: true}\\);) >> >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (save) /Rect [20 260 100 280] /P 3 0 R /A << /S /Named /N /Save >> >>",
    ];
    pdf(&objs)
}

fn pdf(objs: &[impl AsRef<str>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o.as_ref()).as_bytes());
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

/// Same-named leaves and similarly prefixed groups expose reset actions that lose ancestors.
fn qualified_reset_fixture(target: &str, exclude: bool) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 10 0 R 13 0 R 14 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 15 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 7 0 R 9 0 R 12 0 R 13 0 R 14 0 R] >>".into(),
        "<< /T (billing) /Kids [5 0 R 8 0 R] >>".into(),
        "<< /T (person) /Parent 4 0 R /Kids [6 0 R 7 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 5 0 R /V (bill) /DV (bill-default) /Rect [20 340 280 360] /AP << /N 18 0 R >> /PCMarker (kept) >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (code) /Parent 5 0 R /V (code) /DV (code-default) /Rect [20 310 280 330] /AP << /N 19 0 R >> /PCMarker (kept) >>".into(),
        "<< /T (personExtra) /Parent 4 0 R /Kids [9 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 8 0 R /V (extra) /DV (extra-default) /Rect [20 280 280 300] /AP << /N 20 0 R >> /PCMarker (kept) >>".into(),
        "<< /T (shipping) /Kids [11 0 R] >>".into(),
        "<< /T (person) /Parent 10 0 R /Kids [12 0 R] >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /Parent 11 0 R /V (ship) /DV (ship-default) /Rect [20 250 280 270] /AP << /N 21 0 R >> /PCMarker (kept) >>".into(),
        "<< /Subtype /Widget /FT /Tx /T (name) /V (root) /DV (root-default) /Rect [20 220 280 240] /AP << /N 22 0 R >> /PCMarker (kept) >>".into(),
        format!("<< /Subtype /Widget /FT /Btn /Ff 65536 /T (reset) /Rect [20 180 100 200] /A << /S /ResetForm /Fields [{target}] /Flags {} >> >>", i32::from(exclude)),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "(billing.person.name)".into(),
        "(billing.person)".into(),
    ];
    for value in ["bill", "code", "extra", "ship", "root"] {
        let content = format!("BT /Helv 10 Tf 0 5 Td ({value}) Tj ET\n");
        objs.push(format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 260 20] /Resources << /Font << /Helv 15 0 R >> >> /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }
    pdf(&objs)
}

#[test]
fn reset_buttons_respect_qualified_field_and_group_references() {
    // Field dictionaries, direct strings and indirect strings identify the same fields.
    let expected_names = ["billing.person.name", "billing.person.code", "billing.personExtra.name", "shipping.person.name", "name"];
    for (target, group) in
        [("6 0 R", false), ("(billing.person.name)", false), ("16 0 R", false), ("5 0 R", true), ("(billing.person)", true), ("17 0 R", true)]
    {
        for exclude in [false, true] {
            let bytes = qualified_reset_fixture(target, exclude);
            let original = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes.clone())).unwrap();
            let appearance = |doc: &pdfcraft_cos::Document, field: &pdfcraft_engine::FormField| {
                let widget = doc.get(field.widgets[0].obj);
                let ap = doc.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
                let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
                let pdfcraft_cos::Object::Stream(stream) = &*normal else { panic!("normal appearance") };
                (stream.decoded().unwrap(), stream.dict.get(b"Resources").cloned())
            };
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("qualified-reset.pdf", None, bytes).unwrap();
                app.set_option("zoom", "150").unwrap();
                app
            });
            h.run_steps(6);
            let original_fields = h.state().session.get(h.state().views[0].id).unwrap().form.clone();
            assert_eq!(
                original_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                expected_names,
                "all original text fields are present"
            );
            click_field(&mut h, "reset");
            let s = h.state();
            let live = s.session.get(s.views[0].id).unwrap();
            let saved = s.session.save_bytes(s.views[0].id).unwrap();
            let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
            let mut reopened_session = pdfcraft_engine::Session::new();
            let reopened_id = reopened_session.open("reopened.pdf", None, saved, None).unwrap();
            let reopened_fields = &reopened_session.get(reopened_id).unwrap().form;
            assert_eq!(
                reopened_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                expected_names,
                "all saved text fields are present"
            );
            for before in original_fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text) {
                let listed = before.name == "billing.person.name" || (group && before.name == "billing.person.code");
                let reset = listed != exclude;
                let expected = if reset { &before.default } else { &before.value };
                let live_field = live.form.iter().find(|f| f.name == before.name).unwrap();
                let after = reopened_fields.iter().find(|f| f.name == before.name).unwrap();
                assert_eq!(&live_field.value, expected, "live {}, target {target}, exclude {exclude}", before.name);
                assert_eq!(&after.value, expected, "saved {}, target {target}, exclude {exclude}", before.name);
                assert_eq!(after.default, before.default, "defaults are preserved");
                assert_eq!(reopened.get(after.obj).as_dict().unwrap().get(b"PCMarker"), original.get(before.obj).as_dict().unwrap().get(b"PCMarker"));
                if !reset {
                    assert_eq!(appearance(&reopened, after), appearance(&original, before), "untargeted appearances are preserved");
                }
            }
        }
    }
}

fn empty_ancestor_reset_fixture(target: &str, exclude: bool) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(qualified_reset_fixture(target, exclude).into()).unwrap();
    for (obj, name) in [(4, ""), (6, "city"), (9, "extra"), (12, "ship"), (13, "other")] {
        doc.update_dict(pdfcraft_cos::ObjRef::new(obj, 0), |d| {
            d.set(b"T".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text(name)));
        })
        .unwrap();
    }
    pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap()
}

#[test]
fn reset_buttons_match_imported_empty_ancestor_names() {
    let names = ["person.city", "person.code", "personExtra.extra", "shipping.person.ship", "other"];
    for (target, group) in [("6 0 R", false), ("5 0 R", true)] {
        for exclude in [false, true] {
            let bytes = empty_ancestor_reset_fixture(target, exclude);
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                app.open_bytes("empty-ancestor-reset.pdf", None, bytes).unwrap();
                app.set_option("zoom", "150").unwrap();
                app
            });
            h.run_steps(6);
            let id = h.state().views[0].id;
            assert_eq!(
                h.state()
                    .session
                    .get(id)
                    .unwrap()
                    .form
                    .iter()
                    .filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text)
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>(),
                names,
                "the importer retains unique names below the empty ancestor"
            );
            click_field(&mut h, "reset");
            let live = h.state().session.get(id).unwrap();
            let saved = h.state().session.save_bytes(id).unwrap();
            let mut reopened = pdfcraft_engine::Session::new();
            let reopened_id = reopened.open("reopened.pdf", None, saved, None).unwrap();
            let after = reopened.get(reopened_id).unwrap();
            assert_eq!(
                after.form.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
                names
            );
            for (name, current, default) in [
                ("person.city", "bill", "bill-default"),
                ("person.code", "code", "code-default"),
                ("personExtra.extra", "extra", "extra-default"),
                ("shipping.person.ship", "ship", "ship-default"),
                ("other", "root", "root-default"),
            ] {
                let selected = name == "person.city" || (group && name == "person.code");
                let expected = if selected != exclude { default } else { current };
                assert_eq!(live.form.iter().find(|f| f.name == name).unwrap().value, [expected], "live {name}, target {target}, exclude {exclude}");
                let field = after.form.iter().find(|f| f.name == name).unwrap();
                assert_eq!(field.value, [expected], "saved {name}, target {target}, exclude {exclude}");
                assert_eq!(field.default, [default], "defaults are unchanged");
            }
        }
    }
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

// Private, source-only diagnostic; never part of the RESET candidate/publication.
#[test]
fn reset_control_click_matches_manual_on_pinned_native_fixture() {
    use pdfcraft_ui_egui::control::ControlClient;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    // Original ignored native fixture, externally SHA-256 pinned in the preparation receipt.
    // Reading must also equal its compile-time bytes; neither path authors or rewrites a fixture.
    const PINNED: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../.codex-state/parity-20261013-recovery/reset-7437-native/field-include.pdf"));
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../.codex-state/parity-20261013-recovery/reset-7437-native/field-include.pdf");
    let bytes = std::fs::read(&source_path).unwrap();
    assert_eq!(bytes.as_slice(), PINNED);
    assert_eq!(bytes.len(), 3767);
    let original = pdfcraft_cos::Document::open(Arc::new(bytes.clone())).unwrap();
    let expected_names = ["billing.person.name", "billing.person.code", "billing.personExtra.name", "shipping.person.name", "name"];
    let expected = [
        ("billing.person.name", "bill", "bill-default"),
        ("billing.person.code", "code", "code-default"),
        ("billing.personExtra.name", "extra", "extra-default"),
        ("shipping.person.name", "ship", "ship-default"),
        ("name", "root", "root-default"),
    ];
    let mut completed = Vec::new();
    for mode in ["control", "manual"] {
        let trace: Arc<Mutex<Vec<Value>>> = Arc::default();
        let trace_for_build = trace.clone();
        let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
        let slot_for_build = slot.clone();
        let input = bytes.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1272.0, 688.0)).build_eframe(move |cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            *slot_for_build.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
            let observer = trace_for_build.clone();
            cc.egui_ctx.on_begin_pass(
                "private-reset-click-input",
                Arc::new(move |ui| {
                    let ctx = ui.ctx();
                    let frame = ctx.cumulative_frame_nr();
                    let pass = ctx.cumulative_pass_nr();
                    let pass_index = ctx.current_pass_index();
                    let row = ctx.input(|i| {
                        json!({
                            "frame": frame, "pass": pass, "pass_index": pass_index,
                            "time": i.time, "raw_time": i.raw.time, "focused": i.focused,
                            "raw_events": format!("{:?}", i.raw.events), "events": format!("{:?}", i.events),
                            "latest_pos": i.pointer.latest_pos().map(|p| [p.x, p.y]),
                            "press_origin": i.pointer.press_origin().map(|p| [p.x, p.y]),
                            "press_start_time": i.pointer.press_start_time(),
                            "primary_down": i.pointer.button_down(egui::PointerButton::Primary),
                            "primary_pressed": i.pointer.button_pressed(egui::PointerButton::Primary),
                            "primary_released": i.pointer.button_released(egui::PointerButton::Primary),
                            "primary_clicked": i.pointer.button_clicked(egui::PointerButton::Primary),
                            "any_click": i.pointer.any_click(), "could_be_click": i.pointer.could_any_button_be_click()
                        })
                    });
                    let mut rows = observer.lock().unwrap();
                    assert!(rows.len() < 128, "bounded private per-pass trace");
                    rows.push(row);
                }),
            );
            app.open_bytes("field-include.pdf", None, input).unwrap();
            app
        });
        h.run_steps(4);
        let client = slot.lock().unwrap().take().unwrap();
        assert!(h.state_mut().execute("view.fit_one_page"));
        h.run_steps(4);
        let id = h.state().views[0].id;
        let doc = h.state().session.get(id).unwrap();
        assert!(!doc.dirty);
        assert_eq!((doc.can_undo(), doc.can_redo()), (None, None));
        let before = doc.form.clone();
        let reset = before.iter().find(|f| f.name == "reset").unwrap();
        assert_eq!(
            reset.button,
            Some(pdfcraft_engine::form_scripts::ButtonAction::Reset { fields: vec!["billing.person.name".into()], exclude: false })
        );
        assert!(!reset.read_only());
        let point = pdfcraft_ui_egui::forms_ui::field_screen_rect(&h.state().views[0], &doc.info, reset, 0).unwrap().center();
        assert_eq!(
            before.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
            expected_names
        );
        for (name, value, default) in expected {
            let f = before.iter().find(|f| f.name == name).unwrap();
            assert_eq!(f.value, [value]);
            assert_eq!(f.default, [default]);
            assert_eq!(
                original.get(f.obj).as_dict().unwrap().get(b"PCMarker"),
                Some(&pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("kept")))
            );
        }
        eprintln!(
            "RESET_DIAGNOSTIC_SETUP {mode} point={point:?} action={:?} mode={:?} tool={:?}",
            reset.button,
            h.state().mode,
            h.state().quick_tool
        );
        trace.lock().unwrap().clear();
        let mut reply = None;
        if mode == "control" {
            let rx = client.send("ui.click", json!({"x": point.x, "y": point.y}));
            let mut answered_at = None;
            for step in 0..30 {
                h.step();
                let live = h.state().session.get(id).unwrap();
                eprintln!(
                    "RESET_DIAGNOSTIC_STEP {mode} {step} dirty={} pending={:?} cursor={:?} fields={:?}",
                    live.dirty,
                    h.state().views[0].pending_edit,
                    h.output().platform_output.cursor_icon,
                    live.form.iter().map(|f| (&f.name, &f.value)).collect::<Vec<_>>()
                );
                if reply.is_none() {
                    if let Ok(r) = rx.try_recv() {
                        eprintln!("RESET_DIAGNOSTIC_REPLY step={step} reply={r:?}");
                        reply = Some(r);
                        answered_at = Some(step);
                    }
                }
                // Protocol completion plus two fixed frames, never a value/dirty-shaped wait.
                if answered_at.is_some_and(|at| step >= at + 2) {
                    break;
                }
            }
        } else {
            h.hover_at(point);
            for step in 0..5 {
                if step == 1 {
                    h.drag_at(point);
                }
                if step == 2 {
                    h.drop_at(point);
                }
                h.step();
                let live = h.state().session.get(id).unwrap();
                eprintln!(
                    "RESET_DIAGNOSTIC_STEP {mode} {step} dirty={} pending={:?} cursor={:?} fields={:?}",
                    live.dirty,
                    h.state().views[0].pending_edit,
                    h.output().platform_output.cursor_icon,
                    live.form.iter().map(|f| (&f.name, &f.value)).collect::<Vec<_>>()
                );
            }
        }
        for row in trace.lock().unwrap().iter() {
            eprintln!("RESET_DIAGNOSTIC_INPUT {mode} {row}");
        }
        completed.push((mode, h, id, before, reply, point));
    }
    // Both fresh paths are observed before a failed control assertion can skip the manual comparison.
    // Check manual first so its strict result is independently established if control then fails.
    for (mode, h, id, before, reply, point) in completed.into_iter().rev() {
        if mode == "control" {
            assert_eq!(reply.expect("one control reply within 30 steps").unwrap()["clicked"], json!([point.x, point.y]));
        }
        assert_eq!((h.state().views.len(), h.state().views[0].id), (1, id));
        let live = h.state().session.get(id).unwrap();
        assert!(live.dirty, "{mode}: a real reset authors the selected field");
        let saved = h.state().session.save_bytes(id).unwrap();
        assert!(saved.starts_with(&bytes), "incremental save retains the complete original prefix");
        let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        let mut session = pdfcraft_engine::Session::new();
        let reopened_id = session.open("reopened.pdf", None, saved, None).unwrap();
        let fields = &session.get(reopened_id).unwrap().form;
        assert_eq!(
            fields.iter().filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Text).map(|f| f.name.as_str()).collect::<Vec<_>>(),
            expected_names
        );
        let appearance = |doc: &pdfcraft_cos::Document, f: &pdfcraft_engine::FormField| {
            let widget = doc.get(f.widgets[0].obj);
            let ap = doc.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
            let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
            let pdfcraft_cos::Object::Stream(stream) = &*normal else { panic!("normal appearance") };
            (stream.decoded().unwrap(), stream.dict.get(b"Resources").cloned())
        };
        for (name, current, default) in expected {
            let wanted = if name == "billing.person.name" { default } else { current };
            let original_field = before.iter().find(|f| f.name == name).unwrap();
            let live_field = live.form.iter().find(|f| f.name == name).unwrap();
            let after = fields.iter().find(|f| f.name == name).unwrap();
            assert_eq!(live_field.value, [wanted], "{mode}: live {name}");
            assert_eq!(after.value, [wanted], "{mode}: saved {name}");
            assert_eq!(after.default, [default], "{mode}: default {name}");
            assert_eq!(
                reopened.get(after.obj).as_dict().unwrap().get(b"PCMarker"),
                original.get(original_field.obj).as_dict().unwrap().get(b"PCMarker")
            );
            if name != "billing.person.name" {
                assert_eq!(appearance(&reopened, after), appearance(&original, original_field), "{mode}: untargeted AP/resources {name}");
            }
        }
    }
}
