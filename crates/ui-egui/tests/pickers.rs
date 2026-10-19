//! Native file pickers never block the frame. A blocking picker ran a nested run loop inside
//! winit's event handler on macOS, which crashed the app when the user clicked File ▸ Open.
//! Commands now return at once, and the chosen files are used on a later frame.

use egui_kittest::Harness;
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{FilePurpose, FileRequest, PdfCraftApp};

/// An `n`-page document with a proper xref table.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn pages(app: &PdfCraftApp, view: usize) -> usize {
    let doc = app.session.get(app.views[view].id).unwrap();
    PageRenderer::new(doc.bytes.clone(), Default::default()).page_count()
}

/// A shell with `docs` open, each the given number of pages.
fn harness(docs: &[usize]) -> Harness<'static, PdfCraftApp> {
    let docs = docs.to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        for (i, n) in docs.iter().enumerate() {
            app.open_bytes(&format!("doc{i}.pdf"), None, fixture(*n)).unwrap();
        }
        app
    });
    h.run_steps(2);
    h
}

#[test]
fn file_open_returns_at_once_and_opens_the_pick_on_the_next_frame() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("picked.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[]);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("file.open"));
    assert!(h.state().views.is_empty(), "the command must not wait for the picker");
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the picked file opens on a later frame");
    assert_eq!(pages(h.state(), 0), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn insert_from_file_uses_the_pick_on_the_next_frame() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-insert-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3]);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("page.insert"));
    assert_eq!(pages(h.state(), 0), 3, "nothing is inserted until the pick arrives");
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pick_for_a_document_that_is_no_longer_active_is_dropped() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-switch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3, 4]);
    h.state_mut().active = Some(0);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("page.insert"));
    // The user switches tabs before the pick arrives.
    h.state_mut().active = Some(1);
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 3, "the document the pick was for is unchanged");
    assert_eq!(pages(h.state(), 1), 4, "the newly active document is unchanged");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the user is told why nothing happened: {toast:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Deliver the same completion queue used by browser file reads, without a real chooser.
fn complete_read(h: &mut Harness<'static, PdfCraftApp>, request: FileRequest) {
    h.state().requests.lock().unwrap().push((request, vec![("source.pdf".into(), fixture(1))]));
    h.run_steps(2);
}

fn texts(app: &PdfCraftApp, index: usize) -> Vec<String> {
    let bytes = app.session.get(app.views[index].id).unwrap().bytes.clone();
    let mut renderer = PageRenderer::new(bytes, Default::default());
    (0..renderer.page_count())
        .map(|page| {
            renderer.render(RenderRequest { page, kind: RequestKind::Text, ..Default::default() }).text.unwrap().plain_text().trim().to_string()
        })
        .collect()
}

#[test]
fn a_delayed_browser_insert_cannot_modify_a_new_active_document() {
    let mut h = harness(&[3]);
    let request = h.state().file_request(FilePurpose::InsertPages).unwrap();
    h.state_mut().open_bytes("other.pdf", None, fixture(1)).unwrap();
    complete_read(&mut h, request);
    assert_eq!(pages(h.state(), 0), 3);
    assert_eq!(pages(h.state(), 1), 1);
    for view in &h.state().views {
        assert!(!h.state().session.get(view.id).unwrap().dirty);
    }
    assert!(h.state().toast.as_ref().unwrap().0.contains("document changed"));
}

#[test]
fn a_delayed_browser_insert_cannot_use_a_closed_tabs_index() {
    let mut h = harness(&[3, 1]);
    h.state_mut().active = Some(0);
    let request = h.state().file_request(FilePurpose::InsertPages).unwrap();
    h.state_mut().close_tab(0);
    complete_read(&mut h, request);
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(pages(h.state(), 0), 1);
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn a_delayed_browser_insert_keeps_its_original_insertion_position() {
    let mut h = harness(&[3]);
    h.state_mut().views[0].select_pages(&[0]);
    let request = h.state().file_request(FilePurpose::InsertPages).unwrap();
    h.state_mut().views[0].select_pages(&[2]);
    complete_read(&mut h, request);
    assert_eq!(texts(h.state(), 0), ["Page 1", "Page 1", "Page 2", "Page 3"]);
    h.state_mut().undo();
    assert_eq!(pages(h.state(), 0), 3);
}

#[test]
fn a_delayed_browser_insert_is_cancelled_after_destination_edits() {
    let mut h = harness(&[3]);
    let request = h.state().file_request(FilePurpose::InsertPages).unwrap();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![2] }));
    complete_read(&mut h, request);
    assert_eq!(texts(h.state(), 0), ["Page 1", "Page 2"]);
    assert!(h.state().toast.as_ref().unwrap().0.contains("document changed"));
}

#[test]
fn a_native_insert_keeps_its_original_insertion_position() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-picker-position-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("source.pdf");
    std::fs::write(&source, fixture(1)).unwrap();
    let mut h = harness(&[3]);
    h.state_mut().views[0].select_pages(&[0]);
    h.state_mut().pick_override = Some(vec![source.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("page.insert"));
    h.state_mut().views[0].select_pages(&[2]);
    h.run_steps(2);
    assert_eq!(texts(h.state(), 0), ["Page 1", "Page 1", "Page 2", "Page 3"]);
    let _ = std::fs::remove_dir_all(dir);
}
