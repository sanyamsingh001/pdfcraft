//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use pdfcraft_engine::{DocId, Edit};
use std::sync::Arc;

use crate::PdfCraftApp;

/// What the user was doing when we asked whether to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this document; tab indexes can change while its prompt is open.
    Tab(DocId),
    /// Quit the application once every dirty document is resolved.
    Quit,
    /// File ▸ Close all: like Quit, but the application stays open.
    All,
}

/// Where Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// The file's own path; asks for one if the document has none (e.g. opened from bytes).
    InPlace,
    /// Always ask for a new location.
    As,
}

/// Native pathless Save already asks for a destination; browser Save always downloads.
/// Both satisfy the same source's script Save As request just like an explicit Save As.
fn satisfies_script_save_as(target: SaveTarget, has_path: bool, browser: bool) -> bool {
    target == SaveTarget::As || !has_path || browser
}

/// Whether a Save wrote successfully, is awaiting its chooser, or failed to start/write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    Completed,
    Pending,
    Failed,
}

/// A deferred close may finish only while this exact user intent remains live.
#[derive(Clone, Debug)]
pub(crate) struct CloseIntent {
    request: CloseRequest,
    token: Arc<()>,
}

/// A native Save/print export waits for a destination without blocking the UI frame.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub(crate) struct SaveRequest {
    pub id: DocId,
    pub generation: u64,
    pub bytes: Arc<Vec<u8>>,
    pub(crate) close_after: Option<CloseIntent>,
    pub print: bool,
}

impl PdfCraftApp {
    /// Apply an edit to the active document. Returns `true` on success; failures are shown.
    pub fn apply_edit(&mut self, edit: Edit) -> bool {
        let Some((i, _)) = self.active_ids() else { return false };
        self.apply_edit_to_view(i, edit)
    }

    /// Apply to an explicitly selected tab, without changing the active document.
    pub(crate) fn apply_edit_to_view(&mut self, i: usize, edit: Edit) -> bool {
        let Some(id) = self.views.get(i).map(|v| v.id) else { return false };
        let Some(out) = self.apply_edit_to_view_collect(i, edit) else { return false };
        self.handle_js(id, out);
        true
    }

    /// Keep script requests local until the caller has settled any form draft it is applying.
    fn apply_edit_to_view_collect(&mut self, i: usize, edit: Edit) -> Option<pdfcraft_engine::js::JsOutput> {
        let id = self.views.get(i)?.id;
        let label = edit.label();
        match self.session.apply(id, edit.clone()) {
            Ok(()) => {
                let Some(doc) = self.session.get(id) else { return Some(Default::default()) };
                let info = &doc.info;
                let view = &mut self.views[i];
                crate::forms_ui::sync_pristine_focus(view, &doc.form);
                match comment_page(&edit) {
                    // Comment edits change one page: keep every other raster.
                    Some(page) => view.page_changed(page),
                    None => view.document_changed(info),
                }
                // Keep the pages the user acted on selected, where they now are.
                match edit {
                    Edit::MovePages { pages, to } => {
                        let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
                        let to = to.min(info.pages.len() - n);
                        view.select_pages(&(to..to + n).collect::<Vec<_>>());
                    }
                    Edit::InsertBlankPage { at, .. } => view.select_pages(&[at.min(info.pages.len() - 1)]),
                    Edit::DeletePages { .. } => view.select_pages(&[]),
                    Edit::AddAnnotation(a) => {
                        // Select the new comment (appended last among the page's comments).
                        let newest = info.annotations.iter().filter(|x| x.page == a.page && x.in_reply_to.is_none()).map(|x| x.index).max();
                        view.comments.selected = newest.map(|n| (a.page, n));
                        view.comments.reveal = true;
                    }
                    Edit::DeleteAnnotation { .. } => view.comments.selected = None,
                    _ => {}
                }
                if std::mem::take(&mut view.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                Some(self.session.take_js_output(id))
            }
            Err(e) => {
                self.notify_fmt("{label} failed: {e}", &[("label", &crate::i18n::action_label(&label)), ("e", &e.to_string())]);
                None
            }
        }
    }

    pub fn undo(&mut self) {
        self.history_step(true);
    }

    pub fn redo(&mut self) {
        self.history_step(false);
    }

    fn history_step(&mut self, undo: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let result = if undo { self.session.undo(id) } else { self.session.redo(id) };
        match result {
            Ok(label) => {
                if let Some(doc) = self.session.get(id) {
                    self.views[i].document_changed(&doc.info);
                    crate::forms_ui::sync_pristine_focus(&mut self.views[i], &doc.form);
                }
                self.views[i].comments.selected = None;
                let label = crate::i18n::action_label(&label);
                if undo {
                    self.notify_fmt("Undid {label}", &[("label", &label)]);
                } else {
                    self.notify_fmt("Redid {label}", &[("label", &label)]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// Apply any edit a view queued this frame (organize toolbar, keys).
    pub(crate) fn process_pending_edits(&mut self) {
        let Some(i) = self.active else { return };
        self.commit_pending_edit_with(i, Self::apply_edit_to_view_collect);
        match self.views.get_mut(i).and_then(|v| v.pending_action.take()) {
            Some(crate::canvas::ViewAction::InsertFromFile) => self.insert_from_file_dialog(),
            Some(crate::canvas::ViewAction::Extract) => self.dialog = Some(crate::Dialog::Extract),
            Some(crate::canvas::ViewAction::Split) => self.dialog = Some(crate::Dialog::Split),
            Some(crate::canvas::ViewAction::CopyPages { cut }) => self.copy_pages(cut),
            Some(crate::canvas::ViewAction::PastePages) => self.paste_pages(),
            None => {}
        }
    }

    fn commit_pending_edit_with(&mut self, index: usize, mut apply: impl FnMut(&mut Self, usize, Edit) -> Option<pdfcraft_engine::js::JsOutput>) {
        let Some(id) = self.views.get(index).map(|view| view.id) else { return };
        let Some(edit) = self.views.get_mut(index).and_then(|view| view.pending_edit.take()) else { return };
        match apply(self, index, edit.clone()) {
            Some(out) => {
                if let Some(view) = self.views.get_mut(index) {
                    view.forms.committing = None;
                }
                self.handle_settled_form_output(id, vec![out], true);
            }
            None => {
                self.recover_form_draft(index, &edit);
                self.handle_settled_form_output(id, Vec::new(), false);
            }
        }
    }

    /// Organize ▸ Copy / Cut: remember the selected pages (the document as it is now); Cut also
    /// deletes them (one page always stays).
    pub fn copy_pages(&mut self, cut: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let pages = self.views[i].target_pages();
        let n = doc.info.pages.len();
        if cut && pages.len() >= n {
            self.notify_tr("A document needs at least one page: copy instead");
            return;
        }
        self.page_clipboard = Some(crate::PageClip { name: doc.name.clone(), bytes: doc.bytes.clone(), pages: pages.clone() });
        if cut {
            self.apply_edit(Edit::DeletePages { pages: pages.clone() });
        }
        let what = if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) };
        if cut {
            self.notify_fmt("Cut {what}", &[("what", &what)]);
        } else {
            self.notify_fmt("Copied {what}", &[("what", &what)]);
        }
    }

    /// Organize ▸ Paste: insert the copied pages after the selection (or the current page).
    pub fn paste_pages(&mut self) {
        let Some(clip) = self.page_clipboard.clone() else {
            self.notify_tr("Copy or cut pages first");
            return;
        };
        let Some(i) = self.active else { return };
        let at = self.views[i].target_pages().into_iter().max().map_or(0, |p| p + 1);
        let count = clip.pages.len();
        if self.apply_edit(Edit::InsertPagesFrom { name: clip.name.clone(), bytes: clip.bytes.clone(), pages: Some(clip.pages.clone()), at }) {
            self.views[i].select_pages(&(at..at + count).collect::<Vec<_>>());
        }
    }

    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, target: SaveTarget) -> bool {
        self.save_active_outcome(target) == SaveOutcome::Completed
    }

    pub(crate) fn save_active_outcome(&mut self, target: SaveTarget) -> SaveOutcome {
        match self.active {
            Some(i) => self.save_view_then(i, target, None),
            None => SaveOutcome::Failed,
        }
    }

    /// Reset intentionally discards draft input, but only after the model accepts it.
    pub(crate) fn reset_form_view(&mut self, index: usize) -> bool {
        self.reset_form_view_with(index, Self::apply_edit_to_view_collect)
    }

    fn reset_form_view_with(&mut self, index: usize, mut apply: impl FnMut(&mut Self, usize, Edit) -> Option<pdfcraft_engine::js::JsOutput>) -> bool {
        let Some(id) = self.views.get(index).map(|view| view.id) else { return false };
        let Some(output) = apply(self, index, Edit::ResetForm { names: None }) else { return false };
        if let Some(view) = self.views.get_mut(index) {
            view.forms.focus = None;
            view.forms.committing = None;
            view.pending_edit = None;
        }
        self.handle_settled_form_output(id, vec![output], true);
        true
    }

    /// Flush this tab's form edits. Failure keeps the rejected input editable.
    pub(crate) fn flush_form_view(&mut self, index: usize) -> bool {
        self.flush_form_view_with(index, Self::apply_edit_to_view_collect)
    }

    fn flush_form_view_with(&mut self, index: usize, mut apply: impl FnMut(&mut Self, usize, Edit) -> Option<pdfcraft_engine::js::JsOutput>) -> bool {
        let Some(id) = self.views.get(index).map(|view| view.id) else { return false };
        let mut output = Vec::new();
        let accepted = 'settle: {
            // A blur may already have queued a field edit. Keep its draft recoverable until
            // the engine accepts it, and hold all script output until both drafts settle.
            if let Some(edit) = self.views.get(index).and_then(|view| view.pending_edit.clone()) {
                let Some(out) = apply(self, index, edit.clone()) else {
                    self.recover_form_draft(index, &edit);
                    break 'settle false;
                };
                output.push(out);
                if let Some(view) = self.views.get_mut(index) {
                    view.pending_edit = None;
                    view.forms.committing = None;
                }
            }
            let Some(doc) = self.session.get(id) else { break 'settle false };
            let Some(view) = self.views.get(index) else { break 'settle false };
            let draft = match crate::forms_ui::focused_edit(view, &doc.form) {
                Ok(edit) => edit,
                Err(e) => {
                    let name = doc.name.clone();
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e)]);
                    break 'settle false;
                }
            };
            if let Some(edit) = draft {
                let Some(out) = apply(self, index, edit) else { break 'settle false };
                output.push(out);
            }
            if let Some(view) = self.views.get_mut(index) {
                view.forms.focus = None;
            }
            true
        };
        self.handle_settled_form_output(id, output, accepted);
        accepted
    }

    /// Save the document shown in tab `index`. `false` also means a chooser is still pending.
    pub fn save_view(&mut self, index: usize, target: SaveTarget) -> bool {
        self.save_view_then(index, target, None) == SaveOutcome::Completed
    }

    fn save_view_then(&mut self, index: usize, target: SaveTarget, close_after: Option<CloseIntent>) -> SaveOutcome {
        let id = self.views.get(index).map(|view| view.id);
        let has_path = id.and_then(|id| self.session.get(id)).is_some_and(|doc| doc.path.is_some());
        let satisfies_script = satisfies_script_save_as(target, has_path, cfg!(target_arch = "wasm32"));
        let outcome = self.save_view_then_inner(index, target, close_after);
        // A destination request/download already accepted by the outer operation also
        // satisfies its validation scripts. A failed chooser/write leaves the request live.
        if satisfies_script && outcome != SaveOutcome::Failed {
            self.pending_script_saves.retain(|pending| Some(*pending) != id);
        }
        outcome
    }

    fn save_view_then_inner(&mut self, index: usize, target: SaveTarget, close_after: Option<CloseIntent>) -> SaveOutcome {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return SaveOutcome::Failed };
        if !self.flush_form_view(index) {
            return SaveOutcome::Failed;
        }
        let Some(doc) = self.session.get(id) else { return SaveOutcome::Failed };
        let name = doc.name.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let (path, generation) = (doc.path.clone(), doc.edit_generation());
        let bytes = match self.session.save_bytes(id) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return SaveOutcome::Failed;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let request = SaveRequest { id, generation, bytes, close_after, print: false };
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(path)) => Some(path.clone()),
                (SaveTarget::InPlace, Some(path), _) => Some(path),
                _ => None,
            };
            if let Some(destination) = destination {
                return if self.complete_native_save(request, &destination) { SaveOutcome::Completed } else { SaveOutcome::Failed };
            }
            let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
            if self.pick_save(request, &name) { SaveOutcome::Pending } else { SaveOutcome::Failed }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = target;
            if let Err(e) = download(&name, &bytes) {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e)]);
                return SaveOutcome::Failed;
            }
            if let Err(e) = self.session.mark_saved(id, bytes, None) {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                return SaveOutcome::Failed;
            }
            if let Some(doc) = self.session.get(id)
                && let Some(view) = self.views.get_mut(index)
            {
                view.document_changed(&doc.info);
            }
            self.notify_fmt("Downloaded {name}", &[("name", &name)]);
            if let Some(request) = close_after
                && let Some(ctx) = self.ctx.clone()
            {
                self.finish_close_if_live(&ctx, index, request);
            }
            SaveOutcome::Completed
        }
    }

    /// Complete a native destination choice only for the unchanged original document.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn complete_native_save(&mut self, request: SaveRequest, destination: &str) -> bool {
        let Some(index) = self.views.iter().position(|view| view.id == request.id) else {
            self.notify_fmt(
                "Couldn't save {name}: {e}",
                &[("name", destination), ("e", "the document was closed while choosing a destination; no file was written")],
            );
            return false;
        };
        let unchanged =
            self.session.get(request.id).is_some_and(|doc| doc.edit_generation() == request.generation) && !self.view_has_form_draft(index);
        if !unchanged {
            let action = if request.print { "print" } else { "save" };
            let error = format!("the document changed while choosing a destination; {action} again");
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", destination), ("e", &error)]);
            return false;
        }
        if let Err(e) = write_atomically(destination, &request.bytes) {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", destination), ("e", &e.to_string())]);
            return false;
        }
        if request.print {
            self.notify_fmt("Saved the print-ready PDF to {path}", &[("path", destination)]);
            return true;
        }
        if let Err(e) = self.session.mark_saved(request.id, request.bytes, Some(destination.to_string())) {
            self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
            return false;
        }
        self.forget_recovery(request.id);
        if let Some(doc) = self.session.get(request.id)
            && let Some(view) = self.views.get_mut(index)
        {
            view.document_changed(&doc.info);
        }
        self.notify_fmt("Saved {name}", &[("name", &short_name(destination))]);
        if let Some(close) = request.close_after
            && let Some(ctx) = self.ctx.clone()
        {
            self.finish_close_if_live(&ctx, index, close);
        }
        true
    }

    /// Restore rejected input as an editable draft instead of retrying/dropping it each frame.
    fn recover_form_draft(&mut self, index: usize, edit: &Edit) {
        let original = match edit {
            Edit::SetFieldValue { name, .. } => self
                .views
                .get(index)
                .and_then(|view| self.session.get(view.id))
                .and_then(|doc| doc.form.iter().find(|field| field.name == *name))
                .map(|field| field.value.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let Some(view) = self.views.get_mut(index) else { return };
        if let Some(recovered) = view.forms.committing.take() {
            view.forms.focus = Some(crate::forms_ui::Focus { request_focus: true, ..recovered });
            view.pending_edit = None;
            return;
        }
        let Edit::SetFieldValue { name, value } = edit else { return };
        let (text, picked) = match value {
            pdfcraft_engine::FieldValue::Text(text) => (text.clone(), Vec::new()),
            pdfcraft_engine::FieldValue::Choice(picked) => (String::new(), picked.clone()),
            _ => return,
        };
        view.forms.focus = Some(crate::forms_ui::Focus {
            name: name.clone(),
            widget: 0,
            text,
            picked,
            original,
            request_focus: true,
            select_all: false,
            calendar: None,
            calendar_rect: None,
        });
        view.pending_edit = None;
    }

    /// Drafts count as unsaved work without making a merely focused field dirty.
    pub(crate) fn view_has_unsaved_work(&self, index: usize) -> bool {
        let Some(view) = self.views.get(index) else { return false };
        let Some(doc) = self.session.get(view.id) else { return false };
        doc.dirty || self.view_has_form_draft(index)
    }

    pub(crate) fn view_has_form_draft(&self, index: usize) -> bool {
        let Some(view) = self.views.get(index) else { return false };
        let Some(doc) = self.session.get(view.id) else { return false };
        view.pending_edit.is_some()
            || view.forms.committing.is_some()
            || crate::forms_ui::focused_edit(view, &doc.form).map_or(true, |edit| edit.is_some())
    }

    /// Close a tab, asking first if it has unsaved changes.
    pub fn request_close_tab(&mut self, index: usize) {
        let dirty = self.view_has_unsaved_work(index);
        if dirty {
            if let Some(id) = self.views.get(index).map(|view| view.id) {
                self.begin_close(CloseRequest::Tab(id));
            }
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    pub fn close_all(&mut self) {
        for i in (0..self.views.len()).rev() {
            if !self.view_has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        if self.first_dirty().is_some() {
            self.begin_close(CloseRequest::All);
        }
    }

    /// File ▸ Revert (after the confirmation).
    pub fn revert_active(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.revert(id) {
            Ok(()) => {
                if let Some(i) = self.active
                    && let Some(d) = self.session.get(id)
                {
                    let view = &mut self.views[i];
                    view.document_changed(&d.info);
                    // Revert discards this document's unsaved form transaction too. Retaining
                    // a rejected draft would let the next Save write it back into the file.
                    view.forms.focus = None;
                    view.forms.committing = None;
                    view.pending_edit = None;
                }
                self.pending_script_saves.retain(|pending| *pending != id);
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.view_has_unsaved_work(i))
    }

    /// Start a fresh close intent; any older deferred continuation becomes stale.
    fn begin_close(&mut self, request: CloseRequest) {
        self.close_intent = Some(Arc::new(()));
        self.close_request = Some(request);
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(request) = self.close_request.take() else { return };
        // Cancel must revoke a pending close even if its document has since become clean.
        if choice.is_none() {
            self.close_intent = None;
            return;
        }
        let index = match request {
            CloseRequest::Tab(id) => match self.views.iter().position(|view| view.id == id) {
                Some(index) => index,
                None => {
                    self.close_intent = None;
                    return;
                }
            },
            CloseRequest::Quit | CloseRequest::All => match self.first_dirty() {
                Some(index) => index,
                None => {
                    self.close_intent = None;
                    if request == CloseRequest::Quit {
                        self.quit(ctx);
                    }
                    return;
                }
            },
        };
        if choice == Some(true) {
            let token = Arc::new(());
            self.close_intent = Some(token.clone());
            self.ctx = Some(ctx.clone());
            self.save_view_then(index, SaveTarget::InPlace, Some(CloseIntent { request, token }));
        } else {
            self.close_intent = None;
            self.finish_close(ctx, index, request);
        }
    }

    fn finish_close_if_live(&mut self, ctx: &egui::Context, index: usize, intent: CloseIntent) {
        if self.close_intent.as_ref().is_some_and(|current| Arc::ptr_eq(current, &intent.token)) {
            self.finish_close(ctx, index, intent.request);
        }
    }

    fn finish_close(&mut self, ctx: &egui::Context, index: usize, request: CloseRequest) {
        self.close_intent = None;
        self.close_tab(index);
        if request == CloseRequest::Quit || request == CloseRequest::All {
            match self.first_dirty() {
                Some(_) => self.begin_close(request),
                None if request == CloseRequest::Quit => self.quit(ctx),
                None => {}
            }
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Intercept window close while documents have unsaved changes.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.allow_quit && self.first_dirty().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.begin_close(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }
}

/// The page a comment edit changes (`None` for other edits).
fn comment_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::AddAnnotation(a) => Some(a.page),
        Edit::DeleteAnnotation { page, .. }
        | Edit::SetAnnotationContents { page, .. }
        | Edit::ReplyToAnnotation { page, .. }
        | Edit::SetAnnotationStatus { page, .. }
        | Edit::MoveAnnotation { page, .. }
        | Edit::ResizeAnnotation { page, .. }
        | Edit::StyleAnnotation { page, .. }
        | Edit::SetAnnotationInfo { page, .. } => Some(*page),
        _ => None,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = save_file_target(std::path::Path::new(path))?;
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let tmp = dir.join(format!(".{}.pdfcraft-{}.tmp", target.file_name().and_then(|n| n.to_str()).unwrap_or("save"), std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Preserve final file aliases during ordinary local saves, including a missing final file.
/// Keep relative parent links and `..` intact for the filesystem to interpret.
fn save_file_target(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::io::{Error, ErrorKind};
    let mut target = path.to_path_buf();
    let mut seen = std::collections::HashSet::new();
    let limit = || Error::new(ErrorKind::InvalidInput, "save symlink chain is cyclic or exceeds 64 links");
    for depth in 0..=64 {
        if !seen.insert(target.clone()) {
            return Err(limit());
        }
        let symlink = match target.symlink_metadata() {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(e) if e.kind() == ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if symlink {
            if depth == 64 {
                return Err(limit());
            }
            let link = std::fs::read_link(&target)?;
            target = if link.is_absolute() {
                link
            } else {
                target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new(".")).join(link)
            };
            continue;
        }
        let parent = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        if !parent.metadata()?.is_dir() {
            return Err(Error::new(ErrorKind::InvalidInput, "save parent is not a folder"));
        }
        return Ok(target);
    }
    Err(limit())
}

#[cfg(not(target_arch = "wasm32"))]
fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Offer bytes as a browser download.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

impl PdfCraftApp {
    /// Carry out a Bookmarks-panel action as an undoable edit.
    pub fn bookmark_action(&mut self, action: crate::panels::BmAction) {
        use crate::panels::BmAction as A;
        let Some((i, id)) = self.active_ids() else { return };
        let current = self.views[i].current;
        let parent_of = |p: &[usize]| p[..p.len() - 1].to_vec();
        let edit = match action {
            A::New => {
                let n = self.session.get(id).map_or(0, |d| d.info.outline.len());
                self.apply_edit(Edit::AddBookmark { parent: vec![], index: n, title: "Untitled".into(), page: current });
                // Like Acrobat: the new bookmark starts in rename mode.
                self.bookmark_rename = Some((vec![n], "Untitled".into()));
                self.right = Some(crate::RightPanel::Bookmarks);
                return;
            }
            A::StartRename(path) => {
                let title = self.session.get(id).and_then(|d| bookmark_at(&d.info.outline, &path)).map(|b| b.title.clone()).unwrap_or_default();
                self.bookmark_rename = Some((path, title));
                return;
            }
            A::Rename(path, title) => Edit::RenameBookmark { path, title },
            A::SetToCurrentPage(path) => Edit::SetBookmarkPage { path, page: current },
            A::Delete(path) => Edit::DeleteBookmark { path },
            A::MoveUp(path) => {
                let at = path[path.len() - 1].saturating_sub(1);
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::MoveDown(path) => {
                let at = path[path.len() - 1] + 1;
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::Indent(path) => {
                let mut to_parent = parent_of(&path);
                to_parent.push(path[path.len() - 1].saturating_sub(1));
                Edit::MoveBookmark { from: path, to_parent, index: usize::MAX }
            }
            A::Outdent(path) => {
                let parent = parent_of(&path);
                let grand = parent_of(&parent);
                let at = parent[parent.len() - 1] + 1;
                Edit::MoveBookmark { from: path, to_parent: grand, index: at }
            }
        };
        self.apply_edit(edit);
    }
}

fn bookmark_at<'a>(items: &'a [pdfcraft_render::OutlineItem], path: &[usize]) -> Option<&'a pdfcraft_render::OutlineItem> {
    let (first, rest) = path.split_first()?;
    let item = items.get(*first)?;
    if rest.is_empty() { Some(item) } else { bookmark_at(&item.children, rest) }
}

#[cfg(test)]
mod draft_close_tests {
    use super::*;

    #[test]
    fn a_window_close_request_is_intercepted_for_a_focused_form_draft() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        app.views[0].forms.focus = Some(crate::forms_ui::Focus {
            name: "name".into(),
            widget: 0,
            text: "Unsaved Draft".into(),
            picked: Vec::new(),
            original: Vec::new(),
            request_focus: false,
            select_all: false,
            calendar: None,
            calendar_rect: None,
        });
        assert!(!app.session.get(app.views[0].id).unwrap().dirty);
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
        let output = ctx.run_ui(input, |ui| app.guard_quit(ui.ctx()));
        let cancelled_close = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| viewport.commands.contains(&egui::ViewportCommand::CancelClose));
        output.drop_without_applying_deltas();
        assert_eq!(app.close_request, Some(CloseRequest::Quit));
        assert!(cancelled_close);
        app.resolve_close(&ctx, None);
        assert_eq!(app.views.len(), 1, "Cancel keeps the draft's document");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cancelling_a_new_close_intent_revokes_an_older_async_save_continuation() {
        for kind in ["tab", "all", "quit"] {
            let mut app = PdfCraftApp::new();
            app.open_bytes("form.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let id = app.views[0].id;
            assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
            let request = match kind {
                "tab" => CloseRequest::Tab(id),
                "all" => CloseRequest::All,
                _ => CloseRequest::Quit,
            };
            let path = std::env::temp_dir().join(format!("pdfcraft-revoked-close-{kind}-{}.pdf", std::process::id()));
            let _ = std::fs::remove_file(&path);
            app.pick_override = Some(vec![path.to_string_lossy().into_owned()]);
            let ctx = egui::Context::default();
            app.begin_close(request);
            app.resolve_close(&ctx, Some(true));
            assert!(!path.exists(), "the destination completion is still pending");
            assert_eq!(app.views.len(), 1);
            app.begin_close(request);
            app.resolve_close(&ctx, None);
            app.process_picked();
            assert!(path.exists(), "choosing to save may still write its captured bytes");
            assert_eq!(app.views.len(), 1, "Cancel revokes the earlier deferred close");
            assert!(app.session.get(id).is_some());
            assert!(!app.allow_quit, "Cancel must also revoke an earlier Quit continuation");
            assert!(app.close_request.is_none());
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
mod native_save_alias_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let parent = Path::new(".tmp");
            std::fs::create_dir_all(parent).unwrap();
            let path = parent.join(format!("pdfcraft-native-save-alias-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn blank() -> Arc<Vec<u8>> {
        pdfcraft_engine::Session::new().create_blank(200.0, 300.0, 1).unwrap()
    }

    fn assert_rotated(path: &Path) {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("reopened.pdf", None, Arc::new(std::fs::read(path).unwrap()), None).unwrap();
        assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90);
    }

    #[test]
    fn in_place_save_keeps_relative_file_aliases_and_updates_the_reopened_target() {
        for chained in [false, true] {
            let dir = TestDir::new();
            let target = dir.0.join("target.pdf");
            let alias = dir.0.join("alias.pdf");
            let bytes = blank();
            std::fs::write(&target, bytes.as_slice()).unwrap();
            let link = if chained {
                symlink("target.pdf", dir.0.join("middle.pdf")).unwrap();
                "middle.pdf"
            } else {
                "target.pdf"
            };
            symlink(link, &alias).unwrap();
            let mut app = PdfCraftApp::new();
            app.open_path(alias.to_str().unwrap());
            let id = app.active_ids().unwrap().1;
            assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
            assert!(app.save_active(SaveTarget::InPlace));
            assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_link(&alias).unwrap(), Path::new(link));
            if chained {
                assert_eq!(std::fs::read_link(dir.0.join("middle.pdf")).unwrap(), Path::new("target.pdf"));
            }
            assert_ne!(std::fs::read(&target).unwrap(), *bytes);
            assert_rotated(&target);
            assert_rotated(&alias);
            let doc = app.session.get(id).unwrap();
            assert!(!doc.dirty);
            assert_eq!(doc.path.as_deref(), alias.to_str(), "the UI keeps the user's alias pathname");
        }
    }

    #[test]
    fn save_as_keeps_a_dangling_final_alias_and_creates_its_file_target() {
        let dir = TestDir::new();
        std::fs::create_dir(dir.0.join("real")).unwrap();
        let alias = dir.0.join("chosen.pdf");
        let target = dir.0.join("real/new.pdf");
        symlink("real/new.pdf", &alias).unwrap();
        let mut app = PdfCraftApp::new();
        app.open_bytes("unsaved.pdf", None, blank().as_ref().clone()).unwrap();
        let id = app.active_ids().unwrap().1;
        assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.save_override = Some(alias.to_str().unwrap().into());
        assert!(!target.exists());
        assert!(app.save_active(SaveTarget::As));
        assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&alias).unwrap(), Path::new("real/new.pdf"));
        assert_rotated(&target);
        let doc = app.session.get(id).unwrap();
        assert!(!doc.dirty);
        assert_eq!(doc.path.as_deref(), alias.to_str());
    }

    #[test]
    fn failed_alias_destinations_preserve_the_alias_original_file_and_unsaved_work() {
        for folder in [false, true] {
            let dir = TestDir::new();
            let original = dir.0.join("original.pdf");
            let alias = dir.0.join("chosen.pdf");
            let bytes = blank();
            std::fs::write(&original, bytes.as_slice()).unwrap();
            let link = if folder {
                std::fs::create_dir(dir.0.join("folder.pdf")).unwrap();
                std::fs::write(dir.0.join("folder.pdf/sentinel"), b"existing directory content").unwrap();
                "folder.pdf"
            } else {
                "missing-parent/target.pdf"
            };
            symlink(link, &alias).unwrap();
            let mut app = PdfCraftApp::new();
            app.open_path(original.to_str().unwrap());
            let id = app.active_ids().unwrap().1;
            assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
            app.save_override = Some(alias.to_str().unwrap().into());
            assert!(!app.save_active(SaveTarget::As));
            assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_link(&alias).unwrap(), Path::new(link));
            assert_eq!(std::fs::read(&original).unwrap(), *bytes);
            if folder {
                assert_eq!(std::fs::read(dir.0.join("folder.pdf/sentinel")).unwrap(), b"existing directory content");
                assert!(!dir.0.join(format!(".folder.pdf.pdfcraft-{}.tmp", std::process::id())).exists());
            } else {
                assert!(!dir.0.join("missing-parent").exists());
            }
            let doc = app.session.get(id).unwrap();
            assert!(doc.dirty);
            assert_eq!(doc.path.as_deref(), original.to_str());
            assert!(app.toast.as_ref().unwrap().0.contains("Couldn't save"));
        }
    }
}

#[cfg(test)]
mod form_commit_tests {
    use super::*;
    use pdfcraft_engine::FieldValue;
    use pdfcraft_engine::js::{JsOutput, Request};

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.pick_override = Some(Vec::new());
        }
        app
    }

    fn focus(name: &str, text: &str) -> crate::forms_ui::Focus {
        crate::forms_ui::Focus {
            name: name.into(),
            widget: 0,
            text: text.into(),
            picked: Vec::new(),
            original: Vec::new(),
            request_focus: false,
            select_all: false,
            calendar: None,
            calendar_rect: None,
        }
    }

    fn value(name: &str, text: &str) -> Edit {
        Edit::SetFieldValue { name: name.into(), value: FieldValue::Text(text.into()) }
    }

    #[test]
    fn reset_settles_source_drafts_before_delivering_synthetic_output() {
        let mut app = app();
        let source = app.views[0].id;
        app.views[0].forms.focus = Some(focus("name", "Discard focused"));
        app.views[0].forms.committing = Some(focus("city", "Discard queued"));
        app.views[0].pending_edit = Some(value("city", "Discard queued"));
        app.open_bytes("other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let other = app.views[1].id;
        app.views[1].forms.focus = Some(focus("name", "Keep other"));
        // Inject output without running a scripted document or dispatching any save request.
        // Current Reset uses NoScripts; this checks the shared settlement boundary directly.
        assert!(app.reset_form_view_with(0, |app, index, edit| {
            assert_eq!(index, 0);
            assert!(matches!(edit, Edit::ResetForm { names: None }));
            assert!(app.views[index].forms.focus.is_some());
            assert!(app.views[index].forms.committing.is_some());
            assert!(app.views[index].pending_edit.is_some());
            Some(output("reset"))
        }));
        assert!(app.views[0].forms.focus.is_none());
        assert!(app.views[0].forms.committing.is_none());
        assert!(app.views[0].pending_edit.is_none());
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(other));
        assert_eq!(app.views[1].forms.focus.as_ref().unwrap().text, "Keep other");
        assert_eq!(app.pending_script_saves, [source], "one source-owned request waits for a later UI iteration");
        assert!(app.js_console.log.iter().any(|line| line == "reset console"));
        assert!(app.js_console.log.iter().any(|line| line == "Error: reset error"));
        assert_eq!(app.toast.as_ref().unwrap().0, "reset alert");
    }

    // Inject ordinary output at the engine/UI boundary. No document contains a script, and
    // these tests never call the old recursive path or depend on recursion/time limits.
    fn output(label: &str) -> JsOutput {
        JsOutput {
            requests: vec![Request::SaveAs, Request::SaveAs],
            console: vec![format!("{label} console")],
            errors: vec![format!("{label} error")],
            alerts: vec![format!("{label} alert")],
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn flush_settles_pending_and_normalized_focus_before_queuing_source_save() {
        let mut app = app();
        let source = app.views[0].id;
        app.views[0].pending_edit = Some(value("name", "Queued value"));
        app.views[0].forms.committing = Some(focus("name", "Queued value"));
        app.views[0].forms.focus = Some(focus("city", "Focused value"));
        app.open_bytes("foreground.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let foreground = app.views[1].id;
        assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
        let foreground_bytes = app.session.get(foreground).unwrap().bytes.clone();
        let dir = std::env::temp_dir().join(format!("pdfcraft-commit-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settled.pdf");
        let _ = std::fs::remove_file(&path);
        app.pick_override = Some(vec![path.to_string_lossy().into_owned()]);
        let mut calls = 0;
        assert!(app.flush_form_view_with(0, |app, index, edit| {
            calls += 1;
            assert!(calls <= 2, "each draft is applied only once");
            assert!(app.pending_script_saves.is_empty(), "output is held until both drafts settle");
            assert!(!path.exists());
            assert!(app.views[index].forms.focus.is_some());
            let normalized = matches!(&edit, Edit::SetFieldValue { name, .. } if name == "city");
            app.apply_edit_to_view_collect(index, edit)?;
            if normalized {
                // Model the value normalization a successful validation/calculation can make.
                // The focused draft now differs from both its opening and accepted model value.
                app.session.apply(source, value("city", "Normalized value")).unwrap();
                assert!(crate::forms_ui::focused_edit(&app.views[index], &app.session.get(source).unwrap().form).unwrap().is_some());
            }
            Some(output(if normalized { "focused" } else { "pending" }))
        }));
        assert_eq!(calls, 2);
        assert!(app.views[0].pending_edit.is_none());
        assert!(app.views[0].forms.committing.is_none());
        assert!(app.views[0].forms.focus.is_none());
        assert_eq!(app.pending_script_saves, vec![source], "duplicate requests coalesce by source");
        assert_eq!(app.js_console.log, ["pending console", "Error: pending error", "focused console", "Error: focused error"]);
        assert!(!path.exists(), "settlement only queues the request");
        app.process_script_saves();
        assert!(app.pending_script_saves.is_empty());
        assert!(!path.exists(), "chooser completion is still asynchronous");
        app.process_picked();
        let mut reopened = pdfcraft_engine::Session::new();
        let saved = reopened.open("saved.pdf", None, Arc::new(std::fs::read(&path).unwrap()), None).unwrap();
        let form = &reopened.get(saved).unwrap().form;
        assert_eq!(form.iter().find(|field| field.name == "name").unwrap().value, ["Queued value"]);
        assert_eq!(form.iter().find(|field| field.name == "city").unwrap().value, ["Normalized value"]);
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(foreground));
        let doc = app.session.get(foreground).unwrap();
        assert_eq!(doc.bytes, foreground_bytes);
        assert!(doc.dirty && doc.path.is_none());
        assert!(!app.session.get(source).unwrap().dirty);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn rejected_settlement_keeps_drafts_errors_and_cancels_its_save_requests() {
        for reject_pending in [true, false] {
            let mut app = app();
            let id = app.views[0].id;
            app.views[0].pending_edit = Some(value("name", "Queued draft"));
            app.views[0].forms.committing = Some(focus("name", "Queued draft"));
            app.views[0].forms.focus = Some(focus("city", "Focused draft"));
            app.handle_settled_form_output(id, vec![output("earlier")], true);
            let mut calls = 0;
            assert!(!app.flush_form_view_with(0, |app, index, edit| {
                calls += 1;
                assert!(calls <= 2);
                if reject_pending || calls == 2 {
                    app.notify("Field draft rejected");
                    None
                } else {
                    app.apply_edit_to_view_collect(index, edit)?;
                    Some(output("accepted pending"))
                }
            }));
            assert_eq!(calls, if reject_pending { 1 } else { 2 });
            assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, if reject_pending { "Queued draft" } else { "Focused draft" });
            assert!(app.views[0].forms.committing.is_none());
            assert!(app.views[0].pending_edit.is_none());
            assert!(app.pending_script_saves.is_empty());
            assert_eq!(app.toast.as_ref().unwrap().0, "Field draft rejected");
            assert!(app.js_console.log.iter().any(|line| line == "Error: earlier error"));
            if !reject_pending {
                assert!(app.js_console.log.iter().any(|line| line == "Error: accepted pending error"));
            }
            assert!(app.js_console.log.last().unwrap().contains("Save As request was cancelled"));
        }
    }

    #[test]
    fn blur_commit_clears_committing_before_releasing_output_and_keeps_the_next_draft() {
        let mut app = app();
        let id = app.views[0].id;
        app.views[0].pending_edit = Some(value("name", "Queued draft"));
        app.views[0].forms.committing = Some(focus("name", "Queued draft"));
        app.views[0].forms.focus = Some(focus("city", "Next draft"));
        let mut calls = 0;
        app.commit_pending_edit_with(0, |app, index, edit| {
            calls += 1;
            assert!(app.views[index].forms.committing.is_some());
            assert!(app.pending_script_saves.is_empty());
            app.apply_edit_to_view_collect(index, edit)?;
            Some(output("blur"))
        });
        assert_eq!(calls, 1);
        assert!(app.views[0].forms.committing.is_none());
        assert!(app.views[0].pending_edit.is_none());
        assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Next draft");
        assert_eq!(app.pending_script_saves, vec![id]);
        assert_eq!(app.js_console.log, ["blur console", "Error: blur error"]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn accepted_outer_save_as_coalesces_requests_but_failed_output_keeps_them() {
        let mut app = app();
        let id = app.views[0].id;
        app.handle_settled_form_output(id, vec![output("pending")], true);
        assert_eq!(app.save_view_then(0, SaveTarget::As, None), SaveOutcome::Pending);
        assert!(app.pending_script_saves.is_empty());
        app.process_picked(); // synthetic chooser cancellation; no file or native picker
        let dir = std::env::temp_dir().join(format!("pdfcraft-commit-save-failure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        app.save_override = Some(dir.to_string_lossy().into_owned());
        app.handle_settled_form_output(id, vec![output("retry")], true);
        assert_eq!(app.save_view_then(0, SaveTarget::As, None), SaveOutcome::Failed, "a directory cannot be replaced by a saved PDF");
        assert_eq!(app.pending_script_saves, vec![id]);
        assert!(app.toast.as_ref().unwrap().0.contains("Couldn't save"));
        assert!(app.session.get(id).unwrap().path.is_none());
        let file = dir.join("accepted.pdf");
        app.save_override = Some(file.to_string_lossy().into_owned());
        assert_eq!(app.save_view_then(0, SaveTarget::As, None), SaveOutcome::Completed);
        assert!(app.pending_script_saves.is_empty());
        assert_eq!(app.session.get(id).unwrap().path.as_deref(), file.to_str());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn successful_revert_discards_only_its_sources_form_and_save_transaction() {
        let mut app = app();
        let source = app.views[0].id;
        assert!(app.apply_edit(value("name", "Committed change")));
        app.views[0].forms.focus = Some(focus("name", "Focused draft"));
        app.views[0].forms.committing = Some(focus("city", "Committing draft"));
        app.views[0].pending_edit = Some(value("city", "Queued draft"));
        app.handle_settled_form_output(source, vec![output("source")], true);
        app.open_bytes("other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let other = app.views[1].id;
        assert!(app.apply_edit(value("name", "Other committed change")));
        app.views[1].forms.focus = Some(focus("name", "Other focused draft"));
        app.views[1].forms.committing = Some(focus("city", "Other committing draft"));
        app.views[1].pending_edit = Some(value("city", "Other queued draft"));
        app.handle_settled_form_output(other, vec![output("other")], true);
        let other_bytes = app.session.get(other).unwrap().bytes.clone();
        let other_generation = app.session.get(other).unwrap().edit_generation();
        let source_generation = app.session.get(source).unwrap().edit_generation();
        app.active = Some(0);

        app.revert_active();

        let reverted = app.session.get(source).unwrap();
        assert!(!reverted.dirty);
        assert_eq!(reverted.edit_generation(), source_generation + 1, "captured destinations become stale");
        assert!(reverted.form.iter().find(|field| field.name == "name").unwrap().value.is_empty());
        assert!(app.views[0].forms.focus.is_none());
        assert!(app.views[0].forms.committing.is_none());
        assert!(app.views[0].pending_edit.is_none());
        assert!(!app.view_has_unsaved_work(0));
        assert_eq!(app.pending_script_saves, [other]);
        assert_eq!(app.views[1].forms.focus.as_ref().unwrap().text, "Other focused draft");
        assert_eq!(app.views[1].forms.committing.as_ref().unwrap().text, "Other committing draft");
        assert!(
            matches!(&app.views[1].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "city" && text == "Other queued draft")
        );
        let untouched = app.session.get(other).unwrap();
        assert_eq!(untouched.bytes, other_bytes);
        assert_eq!(untouched.edit_generation(), other_generation);
        assert!(untouched.dirty);
        assert_eq!(app.active_ids(), Some((0, source)));
    }

    #[test]
    fn failed_revert_keeps_form_drafts_pending_edits_and_source_save_requests() {
        let mut app = app();
        let source = app.views[0].id;
        app.views[0].forms.focus = Some(focus("name", "Focused draft"));
        app.views[0].forms.committing = Some(focus("city", "Committing draft"));
        app.views[0].pending_edit = Some(value("city", "Queued draft"));
        app.handle_settled_form_output(source, vec![output("source")], true);
        // A bounded engine refusal: the view's source is no longer available. No scripts,
        // corrupt-document artifact or failed filesystem operation are needed for this path.
        app.session.close(source);

        app.revert_active();

        assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Focused draft");
        assert_eq!(app.views[0].forms.committing.as_ref().unwrap().text, "Committing draft");
        assert!(
            matches!(&app.views[0].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "city" && text == "Queued draft")
        );
        assert_eq!(app.pending_script_saves, [source]);
        assert_eq!(app.toast.as_ref().unwrap().0, pdfcraft_engine::EditError::NoDocument.to_string());
    }

    #[test]
    fn script_save_as_coalescing_matches_native_and_browser_save_behavior() {
        for target in [SaveTarget::InPlace, SaveTarget::As] {
            for has_path in [false, true] {
                assert!(satisfies_script_save_as(target, has_path, true), "browser Save always downloads");
                assert_eq!(satisfies_script_save_as(target, has_path, false), target == SaveTarget::As || !has_path);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn pathless_in_place_save_coalesces_its_pending_script_destination_request() {
        let mut app = app();
        let source = app.views[0].id;
        app.open_bytes("foreground.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        let foreground = app.views[1].id;
        app.handle_settled_form_output(source, vec![output("source")], true);
        app.handle_settled_form_output(foreground, vec![output("foreground")], true);
        assert_eq!(app.save_view_then(0, SaveTarget::InPlace, None), SaveOutcome::Pending);
        assert_eq!(app.pending_script_saves, vec![foreground], "only the accepted destination request's source coalesces");
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(foreground));
        app.process_picked(); // synthetic cancellation; never opens a native chooser or writes
        assert!(app.session.get(source).unwrap().path.is_none());
        assert_eq!(app.pending_script_saves, vec![foreground], "cancellation must not revive an already consumed duplicate");
        assert!(!app.toast.as_ref().is_some_and(|toast| toast.0.contains("Couldn't")));
    }
}
