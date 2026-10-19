//! Scratch owned by one test, under Cargo artifacts rather than the source checkout.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        // Cargo supplies this for integration tests. Unit tests use an artifact-local fallback
        // because CARGO_TARGET_TMPDIR is not defined for library test targets.
        let base = option_env!("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().join(".tmp"))
            .join("pdfcraft-cli");
        std::fs::create_dir_all(&base).unwrap();
        for _ in 0..128 {
            let path = base.join(format!("{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path.canonicalize().unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create test scratch {}: {error}", path.display()),
            }
        }
        panic!("could not reserve a unique test scratch directory");
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Only this exclusively created child is ours. Never clear a shared parent or a
        // pre-existing candidate; on test failure this also cleans partially written fixtures.
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
            && !std::thread::panicking()
        {
            panic!("clean test scratch {}: {error}", self.0.display());
        }
    }
}
