//! Ordinary automation output is atomic, with exclusive staging and unchanged confinement.

#[path = "support/scratch.rs"]
mod scratch;
use scratch::ScratchDir;

use pdfcraft_automation::Automation;
#[cfg(unix)]
use pdfcraft_automation::ToolError;
#[cfg(unix)]
use std::path::Path;

fn workdir(name: &str) -> ScratchDir {
    ScratchDir::new(&format!("automation-output-{name}"))
}

#[test]
fn ordinary_output_overwrites_completely_without_touching_the_old_staging_name() {
    let dir = workdir("ordinary");
    let path = dir.join("result.bin");
    let old_temp = dir.join(".result.bin.pdfcraft-tmp");
    std::fs::write(&path, b"old contents longer than the replacement").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    std::fs::write(&old_temp, b"unrelated existing file").unwrap();
    let unconfined = Automation::new();
    assert_eq!(unconfined.write_output(path.to_str().unwrap(), b"complete").unwrap(), path);
    assert_eq!(std::fs::read(&path).unwrap(), b"complete");
    let confined = Automation::new().with_root(dir.as_ref()).unwrap();
    assert_eq!(confined.write_output("result.bin", b"second complete output").unwrap(), path);
    assert_eq!(std::fs::read(&path).unwrap(), b"second complete output");
    assert_eq!(std::fs::read(&old_temp).unwrap(), b"unrelated existing file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o640);
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    assert!(confined.write_output(".", b"do not replace root").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"second complete output");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn links_inside_the_root_keep_their_targets_and_links_outside_are_refused() {
    let dir = workdir("links");
    let root = dir.join("root");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let target = root.join("result.bin");
    std::fs::write(&target, b"original inside contents").unwrap();
    let alias = root.join("alias.bin");
    std::os::unix::fs::symlink("result.bin", &alias).unwrap();
    let unconfined = Automation::new();
    assert_eq!(unconfined.write_output(alias.to_str().unwrap(), b"unconfined full output").unwrap(), alias);
    assert_eq!(std::fs::read(&target).unwrap(), b"unconfined full output");
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());

    let marker = outside.join("keep.bin");
    std::fs::write(&marker, b"outside sentinel").unwrap();
    let old_temp = root.join(".result.bin.pdfcraft-tmp");
    std::os::unix::fs::symlink(&marker, &old_temp).unwrap();
    let confined = Automation::new().with_root(&root).unwrap();
    confined.write_output("alias.bin", b"confined full output").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"confined full output");
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(old_temp.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&marker).unwrap(), b"outside sentinel");

    let escape = root.join("escape.bin");
    std::os::unix::fs::symlink(&marker, &escape).unwrap();
    for path in ["escape.bin", "../outside/keep.bin", "../outside/missing/deep.bin"] {
        assert!(
            matches!(confined.write_output(path, b"must not escape"), Err(ToolError::Failed(message)) if message.contains("outside the allowed directory"))
        );
    }
    assert_eq!(std::fs::read(&marker).unwrap(), b"outside sentinel");
    assert!(!outside.join("missing").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn unconfined_device_output_still_accepts_dev_null() {
    let a = Automation::new();
    assert_eq!(a.write_output("/dev/null", b"device output").unwrap(), Path::new("/dev/null"));
}

#[test]
fn atomic_replacement_updates_the_named_file_without_changing_other_hard_links() {
    let dir = workdir("hard-links");
    let target = dir.join("result.bin");
    let other = dir.join("other-link.bin");
    std::fs::write(&target, b"original contents").unwrap();
    std::fs::hard_link(&target, &other).unwrap();
    Automation::new().write_output(target.to_str().unwrap(), b"complete replacement").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"complete replacement");
    assert_eq!(std::fs::read(&other).unwrap(), b"original contents");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn unconfined_dangling_links_create_targets_without_replacing_links_or_normalizing_parent_links() {
    let dir = workdir("dangling");
    std::fs::create_dir(dir.join("sub")).unwrap();
    let alias = dir.join("alias.bin");
    std::os::unix::fs::symlink("sub/next.bin", &alias).unwrap();
    std::os::unix::fs::symlink("../created.bin", dir.join("sub/next.bin")).unwrap();
    let confined = Automation::new().with_root(dir.as_ref()).unwrap();
    assert!(
        matches!(confined.write_output("alias.bin", b"not allowed"), Err(ToolError::Failed(message)) if message.contains("outside the allowed directory"))
    );
    assert!(!dir.join("created.bin").exists());
    let a = Automation::new();
    assert_eq!(a.write_output(alias.to_str().unwrap(), b"complete new contents").unwrap(), alias);
    assert_eq!(std::fs::read(dir.join("created.bin")).unwrap(), b"complete new contents");
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(dir.join("sub/next.bin").symlink_metadata().unwrap().file_type().is_symlink());

    std::fs::create_dir_all(dir.join("actual/nested")).unwrap();
    std::os::unix::fs::symlink("actual/nested", dir.join("parent")).unwrap();
    std::os::unix::fs::symlink("../other.bin", dir.join("actual/nested/leaf.bin")).unwrap();
    std::fs::write(dir.join("other.bin"), b"lexical-parent sentinel").unwrap();
    let leaf = dir.join("parent/leaf.bin");
    a.write_output(leaf.to_str().unwrap(), b"filesystem parent target").unwrap();
    assert_eq!(std::fs::read(dir.join("actual/other.bin")).unwrap(), b"filesystem parent target");
    assert_eq!(std::fs::read(dir.join("other.bin")).unwrap(), b"lexical-parent sentinel");
    assert!(dir.join("actual/nested/leaf.bin").symlink_metadata().unwrap().file_type().is_symlink());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn unconfined_link_cycles_and_invalid_parents_preserve_existing_data_and_link_entries() {
    let dir = workdir("link-errors");
    std::os::unix::fs::symlink("cycle-b.bin", dir.join("cycle-a.bin")).unwrap();
    std::os::unix::fs::symlink("cycle-a.bin", dir.join("cycle-b.bin")).unwrap();
    std::os::unix::fs::symlink("missing/target.bin", dir.join("missing-parent.bin")).unwrap();
    std::fs::write(dir.join("file-parent"), b"parent sentinel").unwrap();
    std::os::unix::fs::symlink("file-parent/target.bin", dir.join("invalid-parent.bin")).unwrap();
    let a = Automation::new();
    for n in 0..65 {
        let target = if n == 64 { "uncreated-deep.bin".to_string() } else { format!("deep-{}.bin", n + 1) };
        std::os::unix::fs::symlink(target, dir.join(format!("deep-{n}.bin"))).unwrap();
    }
    for name in ["cycle-a.bin", "missing-parent.bin", "invalid-parent.bin", "deep-0.bin"] {
        let path = dir.join(name);
        assert!(a.write_output(path.to_str().unwrap(), b"must not write").is_err());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
    }
    assert_eq!(std::fs::read(dir.join("file-parent")).unwrap(), b"parent sentinel");
    assert!(!dir.join("missing").exists());
    assert!(!dir.join("uncreated-deep.bin").exists());
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".pdfcraft-")));
    std::fs::remove_dir_all(dir).unwrap();
}
