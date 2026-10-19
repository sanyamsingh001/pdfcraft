//! Acrobat JavaScript in the shell: what scripts ask for (alerts, printing, navigation, links),
//! the JavaScript console (⌘J), Document JavaScripts, and Preferences ▸ JavaScript.

use egui::{Align, Layout};
use pdfcraft_engine::js::{JsOutput, Request};
use pdfcraft_engine::{DocId, Edit};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The JavaScript console: the input and the output so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsConsole {
    pub input: String,
    pub log: Vec<String>,
}

/// Document JavaScripts: the script being edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocJsDraft {
    pub name: String,
    pub script: String,
}

impl PdfCraftApp {
    /// Act on what scripts produced in document `id`: alerts are shown, console output goes to
    /// the console, print and page requests are carried out, links wait for the user's permission
    /// (form submissions are reported, never sent).
    pub fn handle_js(&mut self, id: DocId, out: JsOutput) {
        if let Some(index) = self.views.iter().position(|view| view.id == id)
            && let Some(doc) = self.session.get(id)
        {
            crate::forms_ui::sync_pristine_focus(&mut self.views[index], &doc.form);
        }
        if out.is_empty() {
            return;
        }
        self.js_console.log.extend(out.console.iter().cloned());
        for e in &out.errors {
            self.js_console.log.push(format!("Error: {e}"));
        }
        let view = self.views.iter().position(|v| v.id == id);
        for r in out.requests {
            match r {
                Request::Print => self.open_print_for(id),
                Request::GoToPage(p) => {
                    if let Some(i) = view {
                        self.views[i].go_to_page(p);
                    }
                }
                Request::LaunchUrl(u) => self.request_document_url(&u, crate::LinkOrigin::Script),
                Request::Submit(u) => self.notify_fmt(
                    "The form asks to be submitted to {u}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                    &[("u", &u)],
                ),
                Request::SaveAs => {
                    if let Some(i) = view {
                        self.save_view(i, crate::SaveTarget::As);
                    }
                }
                Request::Focus(_) | Request::Beep | Request::Reset(_) => {}
            }
        }
        if let Some(a) = out.alerts.last() {
            self.notify(a.clone());
        }
    }

    /// Deliver every commit's output after its drafts have been cleared or recovered. Saving
    /// waits for the UI loop, so validation cannot re-enter the operation that committed it.
    pub(crate) fn handle_settled_form_output(&mut self, id: DocId, output: Vec<JsOutput>, accepted: bool) {
        let failure_toast = (!accepted).then(|| self.toast.clone());
        let mut save = false;
        for mut out in output {
            out.requests.retain(|request| {
                if matches!(request, Request::SaveAs) {
                    save = true;
                    false
                } else {
                    true
                }
            });
            self.handle_js(id, out);
        }
        if accepted {
            if save {
                self.queue_script_save(id);
            }
        } else {
            let queued = self.pending_script_saves.contains(&id);
            self.pending_script_saves.retain(|pending| *pending != id);
            if save || queued {
                self.js_console.log.push(
                    "Error: the form's Save As request was cancelled because a field draft was rejected; correct the field and save again".into(),
                );
            }
        }
        if let Some(toast) = failure_toast {
            self.toast = toast;
        }
    }

    fn queue_script_save(&mut self, id: DocId) {
        if self.views.iter().any(|view| view.id == id) && !self.pending_script_saves.contains(&id) {
            self.pending_script_saves.push(id);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
        }
    }

    /// One batch per UI update, outside the form commit/save/print call stacks. Newly queued
    /// output belongs to the next update; tab changes never change a request's document.
    pub(crate) fn process_script_saves(&mut self) {
        self.dispatch_script_saves_with(|app, index| {
            app.save_view(index, crate::SaveTarget::As);
        });
    }

    fn dispatch_script_saves_with(&mut self, mut save: impl FnMut(&mut Self, usize)) {
        for id in std::mem::take(&mut self.pending_script_saves) {
            if let Some(index) = self.views.iter().position(|view| view.id == id) {
                save(self, index);
            }
        }
    }

    /// The engine has finished this button/console transaction. Deliver its complete output
    /// once, and let the normal settled-form queue flush any remaining UI draft before saving.
    fn handle_script_outcome(&mut self, id: DocId, outcome: pdfcraft_engine::js::Outcome, show_result: bool) {
        let result = if show_result && outcome.error.is_none() { outcome.result } else { None };
        let out =
            JsOutput { alerts: outcome.alerts, console: outcome.console, requests: outcome.requests, errors: outcome.error.into_iter().collect() };
        self.handle_settled_form_output(id, vec![out], true);
        self.js_console.log.extend(result);
    }

    /// Run a push button's JavaScript (its Mouse Up action).
    pub fn run_button_script(&mut self, id: DocId, field: &str, script: &str) {
        match self.session.run_javascript(id, script, Some(field)) {
            Ok(o) => {
                if let Some(i) = self.views.iter().position(|v| v.id == id)
                    && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                {
                    self.views[i].document_changed(&info);
                }
                self.handle_script_outcome(id, o, false);
            }
            Err(e) => self.notify_fmt("{field}: {e}", &[("field", field), ("e", &e.to_string())]),
        }
    }

    /// Prepare a form ▸ detect fields from the page's blanks, lines and boxes.
    pub fn detect_fields(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        match self.session.auto_detect_fields(id, &[]) {
            Ok(names) if names.is_empty() => self.notify_tr("No form fields were detected"),
            Ok(names) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                if names.len() == 1 {
                    self.notify_tr("Detected 1 form field");
                } else {
                    self.notify_fmt("Detected {n} form fields", &[("n", &names.len().to_string())]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The console's Run: evaluate the input in the active document.
    pub fn run_console(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let script = self.js_console.input.clone();
        if script.trim().is_empty() {
            return;
        }
        self.js_console.log.push(format!("> {}", script.trim()));
        match self.session.run_javascript(id, &script, None) {
            Ok(o) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                self.handle_script_outcome(id, o, true);
            }
            Err(e) => self.js_console.log.push(format!("Error: {e}")),
        }
    }
}

fn buttons(ui: &mut egui::Ui, primary: &str, others: &[&str]) -> Option<String> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Stable ids: the console's output above changes how many widgets come first.
            // Labels are translated for display; the returned id stays English.
            if ui.push_id(primary, |ui| widgets::pill_button(ui, tl!(primary), true)).inner.clicked() {
                clicked = Some(primary.to_string());
            }
            for o in others {
                if ui.push_id(o, |ui| widgets::pill_button(ui, tl!(o), false)).inner.clicked() {
                    clicked = Some(o.to_string());
                }
            }
        })
    });
    clicked
}

/// The JavaScript console. Returns `true` to close.
pub(crate) fn console_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("JavaScript Console")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    if !app.session.javascript() {
        ui.label(egui::RichText::new(tl!("JavaScript is turned off (Preferences ▸ JavaScript).")).small().color(t.text_muted));
    }
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(160.0);
            if app.js_console.log.is_empty() {
                ui.label(egui::RichText::new(tl!("Output appears here.")).color(t.text_muted));
            }
            for line in &app.js_console.log {
                ui.label(egui::RichText::new(line).monospace());
            }
        });
    });
    ui.add_space(6.0);
    let input = ui.add(
        egui::TextEdit::multiline(&mut app.js_console.input)
            .code_editor()
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text(tl!("JavaScript, e.g. getField(\"total\").value"))
            .id_salt("js-console-input"),
    );
    let run_key = input.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.add_space(8.0);
    match buttons(ui, "Run", &["Close", "Clear"]).as_deref() {
        Some("Run") => app.run_console(),
        Some("Clear") => app.js_console.log.clear(),
        Some("Close") => return true,
        _ if run_key => app.run_console(),
        _ => {}
    }
    false
}

/// Document JavaScripts: list, edit, add and delete. Returns `true` to close.
pub(crate) fn document_js_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Document JavaScripts")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let scripts = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.document_scripts()).unwrap_or_default();
    let mut edit: Option<Edit> = None;
    ui.horizontal(|ui| {
        ui.label(tl!("Script Name:"));
        ui.add(egui::TextEdit::singleline(&mut app.doc_js.name).desired_width(240.0).id_salt("doc-js-name"));
    });
    ui.add_space(4.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(120.0).id_salt("doc-js-list").show(ui, |ui| {
            ui.set_width(ui.available_width());
            if scripts.is_empty() {
                ui.label(egui::RichText::new(tl!("This document has no document-level scripts.")).color(t.text_muted));
            }
            for (name, js) in &scripts {
                if ui.selectable_label(app.doc_js.name == *name, name).clicked() {
                    app.doc_js = DocJsDraft { name: name.clone(), script: js.clone() };
                }
            }
        });
    });
    ui.add_space(6.0);
    ui.add(egui::TextEdit::multiline(&mut app.doc_js.script).code_editor().desired_rows(8).desired_width(f32::INFINITY).id_salt("doc-js-script"));
    ui.add_space(8.0);
    let name = app.doc_js.name.trim().to_string();
    let close = match buttons(ui, "Save", &["Close", "Delete"]).as_deref() {
        Some("Save") if !name.is_empty() => {
            edit = Some(Edit::SetDocumentScript { name, script: Some(app.doc_js.script.clone()) });
            false
        }
        Some("Delete") if scripts.iter().any(|(n, _)| *n == name) => {
            edit = Some(Edit::SetDocumentScript { name, script: None });
            app.doc_js = DocJsDraft::default();
            false
        }
        Some("Close") => true,
        _ => false,
    };
    if let Some(e) = edit {
        app.apply_edit(e);
    }
    close
}

/// Preferences: interface language, identity and JavaScript. Returns `true` to close.
pub(crate) fn preferences_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Preferences")).font(theme::semibold(18.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Interface language"));
        let selected = crate::i18n::Lang::from_code(&app.language).map_or(tl!("Auto"), crate::i18n::Lang::name);
        let before = app.language.clone();
        egui::ComboBox::from_id_salt("interface-language").selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                ui.close();
            }
            for language in crate::i18n::Lang::all() {
                if ui.selectable_value(&mut app.language, language.code().to_string(), language.name()).clicked() {
                    ui.close();
                }
            }
        });
        // Relabel the rest of this dialog in the new language right away, not next frame.
        if app.language != before {
            crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        }
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Documents and view")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Default workspace mode"));
        for (mode, label) in [
            (crate::Mode::AllTools, "All Tools"),
            (crate::Mode::Read, "Read"),
            (crate::Mode::Edit, "Edit"),
            (crate::Mode::Convert, "Convert"),
            (crate::Mode::Sign, "E-Sign"),
        ] {
            ui.radio_value(&mut app.default_mode, mode, tl!(label));
        }
    });
    ui.label(
        egui::RichText::new(tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(8.0);
    // Identity: the author of new comments (Acrobat: Preferences ▸ Identity).
    ui.label(egui::RichText::new(tl!("Identity")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Name on new comments"));
        ui.add(egui::TextEdit::singleline(&mut app.comment_prefs.author).desired_width(220.0).char_limit(crate::MAX_AUTHOR_CHARS))
            .labelled_by(label.id);
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new("JavaScript").font(theme::semibold(13.0)));
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut on = app.session.javascript();
        if ui.checkbox(&mut on, tl!("Enable Acrobat JavaScript")).changed() {
            app.session.set_javascript(on);
        }
        ui.label(
            egui::RichText::new(tl!("Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."))
                .small()
                .color(t.text_muted),
        );
    });
    ui.add_space(10.0);
    buttons(ui, tl!("OK"), &[]).is_some()
}

#[cfg(test)]
mod deferred_save_tests {
    use super::*;

    #[test]
    fn deferred_save_batch_keeps_source_ids_and_leaves_new_output_for_next_update() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let source = app.views[0].id;
        app.open_bytes("foreground.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let foreground = app.views[1].id;
        app.queue_script_save(source);
        app.queue_script_save(source);
        app.views.swap(0, 1);
        app.active = Some(0);
        let mut calls = Vec::new();
        app.dispatch_script_saves_with(|app, index| {
            calls.push(app.views[index].id);
            assert_eq!(app.views[index].id, source);
            // Model new output arriving during dispatch, without executing any scripts/save.
            app.queue_script_save(foreground);
        });
        assert_eq!(calls, vec![source]);
        assert_eq!(app.pending_script_saves, vec![foreground]);
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(foreground));
        app.close_tab(0);
        app.dispatch_script_saves_with(|_, _| panic!("a closed source must never dispatch to another tab"));
        assert!(app.pending_script_saves.is_empty());
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.views[0].id, source);
    }

    #[test]
    fn deferred_save_requests_repaint_without_dispatching() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let source = app.views[0].id;
        let ctx = egui::Context::default();
        app.ctx = Some(ctx.clone());
        let repaints = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = repaints.clone();
        ctx.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        // Let egui's initial two-pass repaint settle before testing an idle window.
        let mut idle = false;
        for _ in 0..4 {
            let frame = ctx.run_ui(egui::RawInput::default(), |_| {});
            idle = frame.viewport_output.get(&egui::ViewportId::ROOT).unwrap().repaint_delay > std::time::Duration::ZERO;
            frame.drop_without_applying_deltas();
        }
        assert!(idle);
        repaints.store(0, std::sync::atomic::Ordering::SeqCst);
        app.queue_script_save(source);
        assert_eq!(app.pending_script_saves, vec![source]);
        assert!(repaints.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(app.session.get(source).unwrap().path.is_none());
    }

    #[test]
    fn button_and_console_outcomes_defer_source_save_and_deliver_notices_once() {
        for (show_result, error) in [(false, None), (true, None), (true, Some("first error\nsecond error"))] {
            let mut app = PdfCraftApp::new();
            app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let source = app.views[0].id;
            app.views[0].forms.focus = Some(crate::forms_ui::Focus {
                name: "name".into(),
                widget: 0,
                text: "Uncommitted draft".into(),
                picked: Vec::new(),
                original: Vec::new(),
                request_focus: false,
                select_all: false,
                calendar: None,
                calendar_rect: None,
            });
            app.open_bytes("foreground.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let foreground = app.views[1].id;
            let source_bytes = app.session.get(source).unwrap().bytes.clone();
            let foreground_bytes = app.session.get(foreground).unwrap().bytes.clone();
            // This boundary test injects output; it never executes a script, chooser or save.
            app.handle_script_outcome(
                source,
                pdfcraft_engine::js::Outcome {
                    rc: true,
                    alerts: vec!["source notice".into()],
                    console: vec!["source console".into()],
                    requests: vec![Request::SaveAs, Request::SaveAs],
                    error: error.map(str::to_owned),
                    result: Some("7".into()),
                    ..Default::default()
                },
                show_result,
            );
            assert_eq!(app.pending_script_saves, vec![source]);
            assert_eq!(app.toast.as_ref().unwrap().0, "source notice");
            let mut expected = vec!["source console".to_string()];
            if let Some(error) = error {
                expected.push(format!("Error: {error}"));
            } else if show_result {
                expected.push("7".into());
            }
            assert_eq!(app.js_console.log, expected);
            assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Uncommitted draft");
            assert_eq!(app.session.get(source).unwrap().bytes, source_bytes);
            assert_eq!(app.session.get(foreground).unwrap().bytes, foreground_bytes);
            assert_eq!(app.active_ids().map(|(_, id)| id), Some(foreground));
            app.views.swap(0, 1);
            app.active = Some(0);
            let mut calls = Vec::new();
            app.dispatch_script_saves_with(|app, index| calls.push(app.views[index].id));
            app.dispatch_script_saves_with(|_, _| panic!("an outcome must dispatch only once"));
            assert_eq!(calls, vec![source]);
            assert_eq!(app.js_console.log, expected, "dispatch must not replay diagnostics");
            assert!(app.pending_script_saves.is_empty());
        }
    }
}
