use crate::error::{ErrorKind, ProbeError, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

fn npm_native(prefix: &Path) -> Option<PathBuf> {
    let (package, triple) = if cfg!(target_arch = "aarch64") {
        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
    } else {
        ("codex-win32-x64", "x86_64-pc-windows-msvc")
    };
    let root = prefix.join("node_modules/@openai/codex");
    [
        root.join(format!(
            "node_modules/@openai/{package}/vendor/{triple}/bin/codex.exe"
        )),
        prefix.join(format!(
            "node_modules/@openai/{package}/vendor/{triple}/bin/codex.exe"
        )),
        root.join(format!("vendor/{triple}/bin/codex.exe")),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn resolve_candidate(path: PathBuf) -> Result<PathBuf> {
    if !path.is_file() {
        return Err(ProbeError::new(
            ErrorKind::CliNotFound,
            "Codex executable was not found.",
        ));
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return path.canonicalize().map_err(|_| {
            ProbeError::new(ErrorKind::CliNotFound, "Cannot resolve Codex executable.")
        });
    }
    if path
        .file_stem()
        .is_some_and(|v| v.eq_ignore_ascii_case("codex"))
        && let Some(native) = path.parent().and_then(npm_native)
    {
        return resolve_candidate(native);
    }
    Err(ProbeError::new(
        ErrorKind::UnsupportedLauncher,
        "Select a native Codex exe or a supported npm Codex launcher.",
    ))
}

pub fn find_codex(explicit: Option<&Path>, search_path: Option<OsString>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return resolve_candidate(path.to_path_buf());
    }
    if let Some(search_path) = search_path {
        for directory in std::env::split_paths(&search_path).filter(|p| p.is_absolute()) {
            for name in ["codex.exe", "codex.cmd", "codex.ps1"] {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    // Do not silently switch login environments if the first launcher is broken.
                    return resolve_candidate(candidate);
                }
            }
        }
    }
    Err(ProbeError::new(
        ErrorKind::CliNotFound,
        "Codex was not found on PATH; use --codex with a native exe.",
    ))
}
