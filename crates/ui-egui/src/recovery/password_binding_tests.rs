use super::*;
use crate::SaveTarget;
use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_engine::{Edit, Session};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Exclusive-owned scratch beside the test executable, never inside source or system temp.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let executable = std::env::current_exe().unwrap();
        let parent = executable.parent().unwrap().join(".tmp");
        std::fs::create_dir_all(&parent).unwrap();
        for _ in 0..1024 {
            let path = parent.join(format!("recovery-password-binding-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("scratch directory: {e}"),
            }
        }
        panic!("no exclusive scratch directory available");
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Contributor-original encrypted PDFs, distinct pages/text, standard technical font only.
fn encrypted(count: usize, marker: &str, password: &str, seed: u8) -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut kids = Vec::new();
    for _ in 0..count {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::Int).to_vec()));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        let mut fonts = Dict::new();
        fonts.set(b"F0".to_vec(), Object::Dict(font));
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), format!("BT /F0 12 Tf 20 150 Td ({marker}) Tj ET").as_bytes())));
        page.set(b"Contents".to_vec(), Object::Ref(stream));
        kids.push(Object::Ref(doc.add(page)));
    }
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(count as i64));
    })
    .unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: password,
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [seed; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

fn assert_saved_pdf(bytes: Vec<u8>, password: &str, pages: usize, rotation: i64) {
    let mut session = Session::new();
    let id = session.open("saved.pdf", None, Arc::new(bytes), Some(password)).unwrap();
    let doc = session.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), pages);
    assert_eq!(doc.info.pages[0].rotation as i64, rotation);
}

struct Originals {
    _scratch: Scratch,
    store: RecoveryStore,
    meta: RecoveryMeta,
    a_path: String,
    b_path: String,
    a_bytes: Vec<u8>,
    b_bytes: Vec<u8>,
    snapshot: Vec<u8>,
}
impl Originals {
    fn new() -> Self {
        let scratch = Scratch::new();
        let a_path = scratch.0.join("original-A.pdf").to_string_lossy().into_owned();
        let b_path = scratch.0.join("original-B.pdf").to_string_lossy().into_owned();
        let a_bytes = encrypted(4, "Original A", "recovery-password", 2);
        let b_bytes = encrypted(3, "Opened B", "other-password", 3);
        let snapshot = encrypted(2, "Recovered A", "recovery-password", 4);
        std::fs::write(&a_path, &a_bytes).unwrap();
        std::fs::write(&b_path, &b_bytes).unwrap();
        let store = RecoveryStore::new(scratch.0.join("recovery"));
        let meta = RecoveryMeta { key: "recovered-A".into(), name: "same.pdf".into(), path: Some(a_path.clone()), saved_at: 1, encrypted: true };
        store.write(&meta, &snapshot).unwrap();
        Self { _scratch: scratch, store, meta, a_path, b_path, a_bytes, b_bytes, snapshot }
    }
    fn recovering(&self) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.enable_recovery(self.store.clone());
        app.recover(std::slice::from_ref(&self.meta.key));
        assert!(app.views.is_empty() && app.password_prompt.is_some() && app.pending_recovered.is_some());
        app
    }
    fn open_other(&self, app: &mut PdfCraftApp) {
        app.open_bytes("same.pdf", Some(self.b_path.clone()), self.b_bytes.clone()).unwrap();
        assert!(app.password_prompt.is_some());
    }
    fn assert_a_preserved(&self) {
        assert_eq!(std::fs::read(&self.a_path).unwrap(), self.a_bytes);
        assert_eq!(self.store.read(&self.meta.key).unwrap(), self.snapshot);
        assert!(self.store.list().iter().any(|entry| entry.key == self.meta.key));
    }
}

#[test]
fn cancelled_recovery_cannot_redirect_an_unrelated_encrypted_open_or_save() {
    let originals = Originals::new();
    let mut app = originals.recovering();
    app.submit_password(None);
    assert!(app.password_prompt.is_none() && app.pending_recovered.is_none());
    originals.open_other(&mut app);
    app.submit_password(Some("other-password".into()));
    let id = app.active_ids().unwrap().1;
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.path.as_deref(), Some(originals.b_path.as_str()));
    assert!(!doc.dirty);
    assert!(!app.recovery_keys.contains_key(&id));
    assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
    app.autosave_now();
    assert_ne!(app.recovery_keys.get(&id), Some(&originals.meta.key));
    assert_eq!(originals.store.list().len(), 2);
    originals.assert_a_preserved();
    assert!(app.save_active(SaveTarget::InPlace));
    originals.assert_a_preserved();
    assert_eq!(originals.store.list().len(), 1);
    let saved = std::fs::read(&originals.b_path).unwrap();
    assert_ne!(saved, originals.b_bytes);
    assert_saved_pdf(saved, "other-password", 3, 90);
}

#[test]
fn replacing_a_recovery_prompt_detaches_metadata_even_with_the_same_filename() {
    let originals = Originals::new();
    let mut app = originals.recovering();
    let original_prompt = app.password_prompt.as_ref().unwrap().bytes.clone();
    originals.open_other(&mut app);
    assert!(!Arc::ptr_eq(&original_prompt, &app.password_prompt.as_ref().unwrap().bytes));
    assert!(app.pending_recovered.is_none());
    app.submit_password(Some("incorrect-password".into()));
    assert!(app.password_prompt.as_ref().unwrap().error.is_some());
    assert!(app.pending_recovered.is_none());
    app.submit_password(Some("other-password".into()));
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().path.as_deref(), Some(originals.b_path.as_str()));
    assert!(!app.session.get(id).unwrap().dirty);
    assert!(!app.recovery_keys.contains_key(&id));
    app.close_tab(0);
    originals.assert_a_preserved();
    assert_eq!(std::fs::read(&originals.b_path).unwrap(), originals.b_bytes);
}

#[test]
fn recovery_password_retries_retain_only_the_exact_prompt_path_key_and_snapshot_until_save() {
    let originals = Originals::new();
    let mut app = originals.recovering();
    let prompt = app.password_prompt.as_ref().unwrap().bytes.clone();
    app.submit_password(Some("incorrect-password".into()));
    assert!(Arc::ptr_eq(&prompt, &app.password_prompt.as_ref().unwrap().bytes));
    assert!(Arc::ptr_eq(&prompt, &app.pending_recovered.as_ref().unwrap().bytes));
    assert_eq!(app.pending_recovered.as_ref().unwrap().meta, originals.meta);
    originals.assert_a_preserved();
    app.submit_password(Some("recovery-password".into()));
    assert!(app.password_prompt.is_none() && app.pending_recovered.is_none());
    let id = app.active_ids().unwrap().1;
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.path.as_deref(), Some(originals.a_path.as_str()));
    assert!(doc.dirty);
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(app.recovery_keys.get(&id), Some(&originals.meta.key));
    originals.assert_a_preserved();
    assert!(app.save_active(SaveTarget::InPlace));
    assert!(originals.store.list().is_empty());
    assert_saved_pdf(std::fs::read(&originals.a_path).unwrap(), "recovery-password", 2, 0);
    assert_eq!(std::fs::read(&originals.b_path).unwrap(), originals.b_bytes);
}

#[test]
fn terminal_recovery_open_failure_leaves_no_association_for_an_unrelated_password_prompt() {
    let originals = Originals::new();
    originals.store.write(&originals.meta, b"incomplete original recovery snapshot").unwrap();
    let mut app = PdfCraftApp::new();
    app.enable_recovery(originals.store.clone());
    app.recover(std::slice::from_ref(&originals.meta.key));
    assert!(app.views.is_empty() && app.password_prompt.is_none() && app.pending_recovered.is_none());
    originals.open_other(&mut app);
    app.submit_password(Some("other-password".into()));
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().path.as_deref(), Some(originals.b_path.as_str()));
    assert!(!app.recovery_keys.contains_key(&id));
    assert_eq!(originals.store.read(&originals.meta.key).unwrap(), b"incomplete original recovery snapshot");
    assert_eq!(std::fs::read(&originals.a_path).unwrap(), originals.a_bytes);
}

#[test]
fn a_failed_unrelated_open_keeps_the_still_live_recovery_prompt_bound() {
    let originals = Originals::new();
    let mut app = originals.recovering();
    let prompt = app.password_prompt.as_ref().unwrap().bytes.clone();
    assert!(app.open_bytes("unreadable.pdf", None, b"incomplete unrelated file".to_vec()).is_err());
    assert!(Arc::ptr_eq(&prompt, &app.password_prompt.as_ref().unwrap().bytes));
    assert!(Arc::ptr_eq(&prompt, &app.pending_recovered.as_ref().unwrap().bytes));
    app.submit_password(Some("recovery-password".into()));
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().path.as_deref(), Some(originals.a_path.as_str()));
    assert_eq!(app.recovery_keys.get(&id), Some(&originals.meta.key));
    originals.assert_a_preserved();
}
