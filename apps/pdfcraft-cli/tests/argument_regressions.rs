//! CLI regressions for protected text extraction and edit argument validation.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");
/// A PDF with `n` 200×300 pt pages reading "Page 1", "Page 2", …
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn workdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tmp").join(format!("cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}
fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON, got {other:?}"),
    }
}
fn protect(dir: &std::path::Path) {
    std::fs::write(dir.join("plain.pdf"), fixture(3)).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap();
    let doc = call(&mut a, "doc_open", json!({"path":"plain.pdf"}))["doc"].clone();
    call(&mut a, "doc_protect", json!({"doc":doc,"open_password":"openme"}));
    call(&mut a, "doc_save", json!({"doc":doc,"path":"protected.pdf"}));
}
#[test]
fn protected_text_requires_the_password_and_correct_password_reads_all_pages() {
    let dir = workdir("protected-text");
    protect(&dir);
    for password in [None, Some("wrong")] {
        let mut cmd = Command::new(BIN);
        cmd.current_dir(&dir).args(["text", "protected.pdf"]);
        if let Some(password) = password {
            cmd.args(["--password", password]);
        }
        let out = cmd.output().unwrap();
        assert!(!out.status.success(), "opening failure must not exit 0");
        assert!(out.stdout.is_empty(), "opening failure must not emit text");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("password"), "{stderr}");
    }
    let out = Command::new(BIN).current_dir(&dir).args(["text", "protected.pdf", "--password", "openme"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Page 1") && stdout.contains("Page 2") && stdout.contains("Page 3"), "{stdout}");
    assert_eq!(stdout.matches('\u{c}').count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn blank_text_succeeds_and_malformed_document_fails() {
    let dir = workdir("empty-text");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = call(&mut a, "doc_create", json!({"from":"blank"}))["doc"].clone();
    call(&mut a, "doc_save", json!({"doc":doc,"path":"blank.pdf"}));
    let out = Command::new(BIN).current_dir(&dir).args(["text", "blank.pdf"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    std::fs::write(dir.join("broken.pdf"), b"not a PDF").unwrap();
    let out = Command::new(BIN).current_dir(&dir).args(["text", "broken.pdf"]).output().unwrap();
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unknown_edit_options_fail_before_creating_or_replacing_output() {
    let dir = workdir("edit-errors");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    for existing in [false, true] {
        if existing {
            std::fs::write(dir.join("output.pdf"), b"keep this output").unwrap();
        }
        let out = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--out", "output.pdf", "--rotat", "1:90"]).output().unwrap();
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("unknown option") && stderr.contains("--rotat"), "{stderr}");
        if existing {
            assert_eq!(std::fs::read(dir.join("output.pdf")).unwrap(), b"keep this output");
        } else {
            assert!(!dir.join("output.pdf").exists());
        }
    }
    for tail in [&["--title"][..], &["extra.pdf"][..], &["--rotate"][..]] {
        let out = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--out", "output.pdf"]).args(tail).output().unwrap();
        assert!(!out.status.success(), "{tail:?}");
        assert_eq!(std::fs::read(dir.join("output.pdf")).unwrap(), b"keep this output");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn supported_edit_options_and_flag_shaped_values_remain_usable() {
    let dir = workdir("edit-options");
    protect(&dir);
    let out = Command::new(BIN)
        .current_dir(&dir)
        .args([
            "edit",
            "--full",
            "protected.pdf",
            "--out",
            "rotated.pdf",
            "--password",
            "openme",
            "--rotate",
            "1:90",
            "--title",
            "--rotate",
            "--author",
            "Fixture",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("rotated.pdf")).unwrap()), Some("openme")).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    assert_eq!(info.title.as_deref(), Some("--rotate"));
    assert_eq!(info.author.as_deref(), Some("Fixture"));
    let out = Command::new(BIN)
        .current_dir(&dir)
        .args(["edit", "plain.pdf", "--out", "organized.pdf", "--move", "3:1", "--delete", "2", "--insert-blank", "1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("organized.pdf")).unwrap()), None).unwrap();
    assert_eq!(info.pages.len(), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn edit_preserves_dash_prefixed_filenames_and_supports_end_of_options() {
    let dir = workdir("dash-filenames");
    for (input, args) in [
        ("-invoice.pdf", vec!["edit", "-invoice.pdf", "--out", "output.pdf", "--rotate", "1:90"]),
        ("--invoice.pdf", vec!["edit", "--out", "output.pdf", "--rotate", "1:90", "--", "--invoice.pdf"]),
    ] {
        std::fs::write(dir.join(input), fixture(1)).unwrap();
        let out = Command::new(BIN).current_dir(&dir).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("output.pdf")).unwrap()), None).unwrap();
        assert_eq!(info.pages[0].rotation, 90);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn editing_in_place_atomically_replaces_the_input_after_a_successful_edit() {
    let dir = workdir("atomic-overwrite");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    let out = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--out", "input.pdf", "--rotate", "1:90"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("input.pdf")).unwrap()), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn editing_through_an_existing_symlink_preserves_the_link_and_updates_its_target() {
    let dir = workdir("atomic-symlink");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    std::os::unix::fs::symlink("input.pdf", dir.join("alias.pdf")).unwrap();
    let out = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--out", "alias.pdf", "--rotate", "1:90"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(std::fs::symlink_metadata(dir.join("alias.pdf")).unwrap().file_type().is_symlink());
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("input.pdf")).unwrap()), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_edit_and_render_keep_redirected_stdout_append_semantics() {
    const PREFIX: &[u8] = b"KEEP EXISTING PREFIX\n";
    let dir = workdir("direct-stdout");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    for (name, args, pdf) in [
        ("edited.bin", vec!["edit", "input.pdf", "--rotate", "1:90", "--out", "/dev/stdout"], true),
        ("rendered.bin", vec!["render", "input.pdf", "--page", "1", "--dpi", "18", "--out", "/dev/stdout"], false),
    ] {
        let destination = dir.join(name);
        std::fs::write(&destination, PREFIX).unwrap();
        let output = std::fs::OpenOptions::new().append(true).open(&destination).unwrap();
        let result = Command::new(BIN).current_dir(&dir).args(args).stdout(output).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let bytes = std::fs::read(&destination).unwrap();
        assert!(bytes.starts_with(PREFIX), "the existing redirected file prefix must stay");
        let payload = bytes.get(PREFIX.len()..).unwrap();
        if pdf {
            assert!(payload.starts_with(b"%PDF-"));
            let info = pdfcraft_render::inspect(std::sync::Arc::new(payload.to_vec()), None).unwrap();
            assert_eq!(info.pages[0].rotation, 90);
        } else {
            let header = b"P7\nWIDTH 50\nHEIGHT 75\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n";
            assert!(payload.starts_with(header));
            assert_eq!(payload.len(), header.len() + 50 * 75 * 4);
        }
    }
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_edit_output_still_accepts_devices_and_named_fifos() {
    use std::os::unix::fs::FileTypeExt;
    let dir = workdir("direct-special-output");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    let result = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--rotate", "1:90", "--out", "/dev/null"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let fifo = dir.join("output.fifo");
    match Command::new("mkfifo").arg(&fifo).status() {
        Ok(status) => assert!(status.success(), "mkfifo failed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("named FIFO control skipped: POSIX mkfifo is unavailable");
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }
        Err(error) => panic!("mkfifo: {error}"),
    }
    let reader_path = fifo.clone();
    let reader = std::thread::spawn(move || std::fs::read(reader_path));
    let result = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--rotate", "1:90", "--out", "output.fifo"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let bytes = reader.join().unwrap().unwrap();
    assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo(), "the FIFO must remain a FIFO");
    let info = pdfcraft_render::inspect(std::sync::Arc::new(bytes), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}
