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
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut reader = match Command::new("cat").arg(&fifo).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("named FIFO control skipped: POSIX cat is unavailable");
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }
        Err(error) => panic!("cat: {error}"),
    };
    let writer = Command::new(BIN)
        .current_dir(&dir)
        .args(["edit", "input.pdf", "--rotate", "1:90", "--out", "output.fifo"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut writer = match writer {
        Ok(child) => child,
        Err(error) => {
            let _ = reader.kill();
            let _ = reader.wait();
            panic!("CLI writer: {error}");
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timed_out = false;
    loop {
        if reader.try_wait().unwrap().is_some() && writer.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = reader.kill();
            let _ = writer.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let reader = reader.wait_with_output().unwrap();
    let result = writer.wait_with_output().unwrap();
    assert!(!timed_out, "FIFO reader/writer exceeded ten seconds and were terminated");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(reader.status.success(), "{}", String::from_utf8_lossy(&reader.stderr));
    let bytes = reader.stdout;
    assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo(), "the FIFO must remain a FIFO");
    let info = pdfcraft_render::inspect(std::sync::Arc::new(bytes), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_edit_creates_dangling_link_targets_and_respects_filesystem_parent_semantics() {
    let dir = workdir("dangling-links");
    std::fs::write(dir.join("input.pdf"), fixture(1)).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::os::unix::fs::symlink("sub/next.pdf", dir.join("alias.pdf")).unwrap();
    std::os::unix::fs::symlink("../created.pdf", dir.join("sub/next.pdf")).unwrap();
    let result = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--rotate", "1:90", "--out", "alias.pdf"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(dir.join("alias.pdf").symlink_metadata().unwrap().file_type().is_symlink());
    assert!(dir.join("sub/next.pdf").symlink_metadata().unwrap().file_type().is_symlink());
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("created.pdf")).unwrap()), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);

    std::fs::create_dir_all(dir.join("actual/nested")).unwrap();
    std::os::unix::fs::symlink("actual/nested", dir.join("parent")).unwrap();
    std::os::unix::fs::symlink("../other.pdf", dir.join("actual/nested/leaf.pdf")).unwrap();
    std::fs::write(dir.join("other.pdf"), b"lexical-parent sentinel").unwrap();
    let result = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--rotate", "1:90", "--out", "parent/leaf.pdf"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("actual/other.pdf")).unwrap()), None).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    assert_eq!(std::fs::read(dir.join("other.pdf")).unwrap(), b"lexical-parent sentinel");
    assert!(dir.join("actual/nested/leaf.pdf").symlink_metadata().unwrap().file_type().is_symlink());
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_edit_link_cycles_and_invalid_parents_fail_without_altering_files_or_links() {
    let dir = workdir("link-errors");
    let source = fixture(1);
    std::fs::write(dir.join("input.pdf"), &source).unwrap();
    std::os::unix::fs::symlink("cycle-b.pdf", dir.join("cycle-a.pdf")).unwrap();
    std::os::unix::fs::symlink("cycle-a.pdf", dir.join("cycle-b.pdf")).unwrap();
    std::os::unix::fs::symlink("missing/target.pdf", dir.join("missing-parent.pdf")).unwrap();
    std::fs::write(dir.join("file-parent"), b"parent sentinel").unwrap();
    std::os::unix::fs::symlink("file-parent/target.pdf", dir.join("invalid-parent.pdf")).unwrap();
    for n in 0..65 {
        let target = if n == 64 { "uncreated-deep.pdf".to_string() } else { format!("deep-{}.pdf", n + 1) };
        std::os::unix::fs::symlink(target, dir.join(format!("deep-{n}.pdf"))).unwrap();
    }
    for name in ["cycle-a.pdf", "missing-parent.pdf", "invalid-parent.pdf", "deep-0.pdf"] {
        let result = Command::new(BIN).current_dir(&dir).args(["edit", "input.pdf", "--out", name, "--rotate", "1:90"]).output().unwrap();
        assert!(!result.status.success());
        assert!(!result.stderr.is_empty());
        assert!(dir.join(name).symlink_metadata().unwrap().file_type().is_symlink());
    }
    assert_eq!(std::fs::read(dir.join("input.pdf")).unwrap(), source);
    assert_eq!(std::fs::read(dir.join("file-parent")).unwrap(), b"parent sentinel");
    assert!(!dir.join("missing").exists());
    assert!(!dir.join("uncreated-deep.pdf").exists());
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn repeated_edit_output_flags_use_the_last_destination() {
    let dir = workdir("duplicate-out");
    let source = fixture(1);
    std::fs::write(dir.join("input.pdf"), &source).unwrap();
    std::fs::write(dir.join("first.pdf"), b"keep first destination").unwrap();
    for existing in [false, true] {
        if existing {
            std::fs::write(dir.join("last.pdf"), b"replace last destination").unwrap();
        }
        let out = Command::new(BIN)
            .current_dir(&dir)
            .args(["edit", "input.pdf", "--out", "first.pdf", "--out", "last.pdf", "--rotate", "1:90"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read(dir.join("first.pdf")).unwrap(), b"keep first destination");
        assert_eq!(std::fs::read(dir.join("input.pdf")).unwrap(), source);
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("last.pdf")).unwrap()), None).unwrap();
        assert_eq!(info.pages[0].rotation, 90);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn repeated_edit_password_flags_use_the_last_password_and_preserve_failed_output() {
    let dir = workdir("duplicate-password");
    protect(&dir);
    let source = std::fs::read(dir.join("protected.pdf")).unwrap();
    let out = Command::new(BIN)
        .current_dir(&dir)
        .args(["edit", "protected.pdf", "--out", "output.pdf", "--password", "wrong", "--password", "openme", "--rotate", "1:90"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join("output.pdf")).unwrap()), Some("openme")).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    let saved = std::fs::read(dir.join("output.pdf")).unwrap();
    let out = Command::new(BIN)
        .current_dir(&dir)
        .args(["edit", "protected.pdf", "--out", "output.pdf", "--password", "openme", "--password", "wrong", "--rotate", "1:180"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("password"));
    assert_eq!(std::fs::read(dir.join("output.pdf")).unwrap(), saved);
    assert_eq!(std::fs::read(dir.join("protected.pdf")).unwrap(), source);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn zero_page_and_position_arguments_fail_before_reading_or_writing() {
    let dir = workdir("zero-page-preflight");
    let source = fixture(2);
    std::fs::write(dir.join("input.pdf"), &source).unwrap();
    for input in ["input.pdf", "missing.pdf"] {
        for destination in ["output.pdf", "new.pdf", "input.pdf"] {
            std::fs::write(dir.join("output.pdf"), b"keep destination").unwrap();
            for (command, tail, diagnostic) in [
                ("text", vec!["--page", "0"], "--page must be at least 1"),
                ("render", vec!["--page", "0", "--out", destination], "--page must be at least 1"),
                ("edit", vec!["--out", destination, "--move", "1:0"], "--move target must be at least 1"),
                ("edit", vec!["--out", destination, "--insert-blank", "0"], "--insert-blank must be at least 1"),
                ("edit", vec!["--out", destination, "--rotate", "0:90"], "bad page number"),
                ("edit", vec!["--out", destination, "--delete", "0"], "bad page number"),
                ("edit", vec!["--out", destination, "--move", "0:1"], "bad page number"),
                ("extract", vec!["--out", destination, "--pages", "0"], "bad page number"),
                ("split", vec!["--before", "0", "--out-dir", "splits"], "bad page number"),
            ] {
                let out = Command::new(BIN).current_dir(&dir).args([command, input]).args(&tail).output().unwrap();
                assert!(!out.status.success(), "{command} {input} {tail:?}");
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(stderr.contains(diagnostic), "{command} {input} {tail:?}: {stderr}");
                assert!(out.stdout.is_empty());
                assert_eq!(std::fs::read(dir.join("input.pdf")).unwrap(), source);
                assert_eq!(std::fs::read(dir.join("output.pdf")).unwrap(), b"keep destination");
                assert!(!dir.join("new.pdf").exists());
                assert!(!dir.join("missing.pdf").exists());
                assert!(!dir.join("splits").exists());
            }
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "preflight must not create staging files");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn one_based_first_page_and_insertion_position_remain_usable() {
    let dir = workdir("one-based-first-page");
    let source = fixture(2);
    std::fs::write(dir.join("input.pdf"), &source).unwrap();
    let out = Command::new(BIN).current_dir(&dir).args(["text", "input.pdf", "--page", "1"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Page 1") && !text.contains("Page 2"), "{text}");
    let out = Command::new(BIN).current_dir(&dir).args(["render", "input.pdf", "--page", "1", "--dpi", "18", "--out", "page.pam"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let pam = std::fs::read(dir.join("page.pam")).unwrap();
    let header = b"P7\nWIDTH 50\nHEIGHT 75\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n";
    assert!(pam.starts_with(header));
    assert_eq!(pam.len(), header.len() + 50 * 75 * 4);
    let out = Command::new(BIN)
        .current_dir(&dir)
        .args(["edit", "input.pdf", "--out", "inserted.pdf", "--move", "1:1", "--insert-blank", "1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bytes = std::sync::Arc::new(std::fs::read(dir.join("inserted.pdf")).unwrap());
    let info = pdfcraft_render::inspect(bytes, None).unwrap();
    assert_eq!(info.pages.len(), 3);
    assert_eq!((info.pages[0].width, info.pages[0].height), (612.0, 792.0));
    let out = Command::new(BIN).current_dir(&dir).args(["text", "inserted.pdf", "--page", "1"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
    let out = Command::new(BIN).current_dir(&dir).args(["text", "inserted.pdf", "--page", "2"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Page 1"));
    assert_eq!(std::fs::read(dir.join("input.pdf")).unwrap(), source);
    std::fs::remove_dir_all(dir).unwrap();
}
