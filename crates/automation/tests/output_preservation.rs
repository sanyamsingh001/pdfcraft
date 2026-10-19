//! Ordinary automation output is atomic, with exclusive staging and unchanged confinement.

use pdfcraft_automation::Automation;
#[cfg(unix)]
use pdfcraft_automation::ToolError;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

fn workdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tmp").join(format!("automation-output-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
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
    let confined = Automation::new().with_root(&dir).unwrap();
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
