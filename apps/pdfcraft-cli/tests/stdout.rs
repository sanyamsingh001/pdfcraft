//! A closed output pipe is normal when a consumer such as `head` exits early (#135).

use std::io::{Read, pipe};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn closed_stdout(args: &[&str]) -> Output {
    let (reader, writer) = pipe().unwrap();
    drop(reader);
    Command::new(BIN).args(args).stdout(writer).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap()
}

#[test]
fn output_commands_exit_cleanly_when_the_reader_has_closed() {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-stdout-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let pdf = base.join("text.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Pipe test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    let path = pdf.to_str().unwrap();
    let script = base.join("steps.json");
    let saved = base.join("should-not-be-written.pdf");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "blank" } },
        { "tool": "doc_save", "args": { "doc": 1, "path": saved } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    for args in [
        vec!["tools"],
        vec!["--version"],
        vec!["info", path],
        vec!["text", path],
        vec!["check-one", path],
        vec!["run", "command_list"],
        vec!["run", "--script", script.to_str().unwrap()],
        vec!["split", path, "--every", "1", "--out-dir", base.to_str().unwrap()],
    ] {
        let out = closed_stdout(&args);
        assert!(out.status.success(), "{args:?}: {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(!saved.exists(), "a closed output pipe stops the script before the next edit");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn tools_exits_cleanly_when_the_reader_stops_after_one_line() {
    let mut child = Command::new(BIN).arg("tools").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // Read through the first newline without buffering the rest, then close the pipe.
    let mut byte = [0];
    while stdout.read_exact(&mut byte).is_ok() && byte[0] != b'\n' {}
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_closed_stdout_does_not_hide_command_errors() {
    let out = closed_stdout(&["info"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("info: missing file"));
}

#[cfg(target_os = "linux")]
#[test]
fn other_stdout_errors_fail_without_panicking() {
    let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let out = Command::new(BIN).arg("--version").stdout(full).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("stdout") && !stderr.contains("panicked"), "{stderr}");
}

#[test]
fn ordinary_output_keeps_its_format() {
    let out = Command::new(BIN).arg("tools").output().unwrap();
    assert!(out.status.success() && out.stderr.is_empty());
    let tools: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(tools.as_array().unwrap().iter().any(|t| t["name"] == "doc_open"));
    assert!(out.stdout.ends_with(b"\n"));
}

#[cfg(unix)]
#[test]
fn explicit_stdout_document_outputs_exit_cleanly_when_the_reader_has_closed() {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-direct-stdout-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let pdf = base.join("input.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Direct pipe test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    let path = pdf.to_str().unwrap();
    for args in
        [vec!["edit", path, "--rotate", "1:90", "--out", "/dev/stdout"], vec!["render", path, "--page", "1", "--dpi", "18", "--out", "/dev/stdout"]]
    {
        let out = closed_stdout(&args);
        assert!(out.status.success(), "{args:?}: {}: {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_document_file_write_error_still_fails_when_stdout_is_closed() {
    let base = std::env::temp_dir().join(format!("pdfcraft-cli-direct-file-error-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let pdf = base.join("input.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("File error test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    let out = closed_stdout(&["edit", pdf.to_str().unwrap(), "--rotate", "1:90", "--out", "/dev/full"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("/dev/full") && !stderr.contains("panicked"), "{stderr}");
    std::fs::remove_dir_all(base).unwrap();
}

/// Wait only for the CLI child this test owns; a blocked pipe must not hang the suite.
#[cfg(unix)]
fn wait_script(mut child: std::process::Child) -> Output {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!timed_out, "script exceeded thirty seconds and was terminated");
    output
}

#[cfg(unix)]
#[test]
fn a_later_png_stdout_write_keeps_broken_pipe_typed() {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tmp").join(format!("cli-later-png-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // Contributor-original deterministic noise keeps the PNG larger than an ordinary pipe
    // buffer. Closing after its header must interrupt the image write, not the JSON after it.
    let mut rgba = Vec::with_capacity(1024 * 1024 * 4);
    let mut state = 0x1234_5678u32;
    for _ in 0..1024 * 1024 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        rgba.extend_from_slice(&[state as u8, (state >> 8) as u8, (state >> 16) as u8, 255]);
    }
    let png = pdfcraft_engine::export::encode_png(1024, 1024, &rgba).unwrap();
    assert!(png.len() > 2 * 1024 * 1024, "fixture must not collapse to a small blank image");
    let image = base.join("noise.png");
    std::fs::write(&image, png).unwrap();
    let script = base.join("steps.json");
    let saved = base.join("should-not-be-written.pdf");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "images", "paths": [image], "dpi": 72 } },
        { "tool": "page_render", "args": { "doc": 1, "page": 1, "dpi": 72 }, "out": "/dev/stdout" },
        { "tool": "doc_save", "args": { "doc": 1, "path": saved } },
    ]);
    // Check the rendered payload too: a regression that renders a blank page must not
    // accidentally make this exercise only the trailing JSON's broken-pipe handling.
    let mut control = pdfcraft_automation::Automation::new();
    control.call("doc_create", &steps[0]["args"]).unwrap();
    let rendered = control.call("page_render", &steps[1]["args"]).unwrap();
    let [pdfcraft_automation::Content::Png { data, .. }] = rendered.as_slice() else { panic!("expected PNG control") };
    assert!(data.len() > 2 * 1024 * 1024, "rendered fixture must exceed ordinary pipe capacity");
    std::fs::write(&script, steps.to_string()).unwrap();
    let mut child = Command::new(BIN).args(["run", "--script"]).arg(&script).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || -> std::io::Result<(serde_json::Value, [u8; 8])> {
        let mut json = Vec::new();
        let value = loop {
            let mut byte = [0];
            stdout.read_exact(&mut byte)?;
            json.push(byte[0]);
            if json.len() > 65_536 {
                return Err(std::io::Error::other("doc_create reply exceeded the fixture limit"));
            }
            if byte[0] == b'\n'
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&json)
            {
                break value;
            }
        };
        let mut signature = [0; 8];
        stdout.read_exact(&mut signature)?;
        // Drop the pipe only after the complete first JSON and the next PNG's header.
        drop(stdout);
        Ok((value, signature))
    });
    let output = wait_script(child);
    let (created, signature) = reader.join().unwrap().unwrap();
    assert_eq!(created["doc"], 1);
    assert_eq!(&signature, b"\x89PNG\r\n\x1a\n", "the second step must reach PNG output");
    assert!(output.status.success(), "{}: {}", output.status, String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!saved.exists(), "a broken image-output pipe stops the script before its next edit");
    std::fs::remove_dir_all(base).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_later_png_file_error_is_not_treated_as_closed_stdout() {
    let base =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tmp").join(format!("cli-later-png-file-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let script = base.join("steps.json");
    let saved = base.join("should-not-be-written.pdf");
    let steps = serde_json::json!([
        { "tool": "doc_create", "args": { "from": "blank", "width": 20, "height": 20 } },
        { "tool": "page_render", "args": { "doc": 1, "page": 1, "dpi": 18 }, "out": "/dev/full" },
        { "tool": "doc_save", "args": { "doc": 1, "path": saved } },
    ]);
    std::fs::write(&script, steps.to_string()).unwrap();
    let child = Command::new(BIN).args(["run", "--script"]).arg(&script).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let output = wait_script(child);
    let created: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(created["doc"], 1);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("step 2 (page_render)") && stderr.contains("/dev/full") && !stderr.contains("panicked"), "{stderr}");
    assert!(!saved.exists());
    std::fs::remove_dir_all(base).unwrap();
}
