use quotarail::{discovery::find_codex, error::ErrorKind};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "quotarail-discovery-{}-{} 中文 空格",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        // Delete only the newly created direct child of the canonical temp directory.
        if let (Ok(root), Ok(target)) = (std::env::temp_dir().canonicalize(), self.0.canonicalize())
            && target.parent() == Some(root.as_path())
            && target
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("quotarail-discovery-"))
        {
            let _ = std::fs::remove_dir_all(target);
        }
    }
}

#[test]
fn explicit_native_path_with_unicode_and_spaces_is_preserved() {
    let temp = Temp::new();
    let exe = temp.0.join("codex.exe");
    std::fs::write(&exe, b"synthetic path fixture; not executable").unwrap();
    assert_eq!(
        find_codex(Some(&exe), None).unwrap(),
        exe.canonicalize().unwrap()
    );
}

#[test]
fn npm_wrapper_resolves_to_vendor_exe_without_running_shell() {
    let temp = Temp::new();
    let wrapper = temp.0.join("codex.cmd");
    std::fs::write(&wrapper, b"synthetic wrapper; must not execute").unwrap();
    let (package, triple) = if cfg!(target_arch = "aarch64") {
        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
    } else {
        ("codex-win32-x64", "x86_64-pc-windows-msvc")
    };
    let exe = temp.0.join(format!(
        "node_modules/@openai/codex/node_modules/@openai/{package}/vendor/{triple}/bin/codex.exe"
    ));
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"synthetic").unwrap();
    assert_eq!(
        find_codex(Some(&wrapper), None).unwrap(),
        exe.canonicalize().unwrap()
    );
}

#[test]
fn missing_cli_and_unknown_shell_launchers_fail_explicitly() {
    let temp = Temp::new();
    let script = temp.0.join("arbitrary.cmd");
    std::fs::write(&script, b"do not run").unwrap();
    assert_eq!(
        find_codex(Some(&script), None).unwrap_err().kind,
        ErrorKind::UnsupportedLauncher
    );
    assert_eq!(
        find_codex(None, Some(std::ffi::OsString::new()))
            .unwrap_err()
            .kind,
        ErrorKind::CliNotFound
    );
}
