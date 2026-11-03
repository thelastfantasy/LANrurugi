//! Lets a native release package run without configuration: the server finds its frontend,
//! plugins, docs, bundled libraries and models relative to its own install prefix (the parent of
//! the directory holding the real executable). Only variables that are unset are filled, and only
//! with paths that exist, so the container image and explicit settings are never overridden.
//! Layout: `specs/015-native-platforms/contracts/package-layout.md`.

use std::env::consts::{DLL_PREFIX, DLL_SUFFIX};
use std::path::{Path, PathBuf};

/// `(variable, path relative to the install prefix)`.
fn entries() -> Vec<(&'static str, String)> {
    vec![
        ("LANRURUGI_STATIC_DIR", "share/lanrurugi/frontend".into()),
        ("LANRURUGI_PLUGINS_DIR", "share/lanrurugi/plugins".into()),
        ("LANRURUGI_DOCS_DIR", "share/lanrurugi/docs".into()),
        (
            "LANRURUGI_PDFIUM_LIB",
            format!("lib/{DLL_PREFIX}pdfium{DLL_SUFFIX}"),
        ),
        (
            "ORT_DYLIB_PATH",
            format!("lib/{DLL_PREFIX}onnxruntime{DLL_SUFFIX}"),
        ),
        (
            "LANRURUGI_MANGA_OCR_MODEL_DIR",
            "share/lanrurugi/models/manga-ocr".into(),
        ),
        (
            "LANRURUGI_INPAINT_MODEL_DIR",
            "share/lanrurugi/models/inpaint".into(),
        ),
        (
            "LANRURUGI_BUBBLE_SEG_MODEL_DIR",
            "share/lanrurugi/models/bubble-seg".into(),
        ),
    ]
}

/// The install prefix of the running executable, symlinks resolved.
pub fn install_prefix() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    Some(exe.parent()?.parent()?.to_path_buf())
}

/// The variables to fill for `prefix`: each unset one whose install-relative path exists.
pub fn defaults_for(prefix: &Path, is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, PathBuf)> {
    entries()
        .into_iter()
        .filter(|(var, _)| !is_set(var))
        .map(|(var, rel)| (var, prefix.join(rel)))
        .filter(|(_, path)| path.exists())
        .collect()
}

/// Fills the unset location variables from the install prefix and returns what was set.
///
/// Must run before any other thread exists (it calls `std::env::set_var`): first thing in `main`,
/// before the async runtime is built. Child processes inherit the result.
pub fn apply() -> Vec<(&'static str, PathBuf)> {
    let Some(prefix) = install_prefix() else {
        return Vec::new();
    };
    let filled = defaults_for(&prefix, |var| std::env::var_os(var).is_some());
    for (var, path) in &filled {
        std::env::set_var(var, path);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(dirs: &[&str], files: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for dir in dirs {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        for file in files {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        root
    }

    #[test]
    fn fills_only_unset_variables_with_existing_paths() {
        let pdfium = format!("lib/{DLL_PREFIX}pdfium{DLL_SUFFIX}");
        let root = package(
            &["share/lanrurugi/frontend", "share/lanrurugi/plugins"],
            &[&pdfium],
        );
        let filled = defaults_for(root.path(), |var| var == "LANRURUGI_PLUGINS_DIR");
        let vars: Vec<_> = filled.iter().map(|(var, _)| *var).collect();
        // Plugins is already set; docs, ONNX Runtime and models are absent from this package.
        assert_eq!(vars, ["LANRURUGI_STATIC_DIR", "LANRURUGI_PDFIUM_LIB"]);
        assert_eq!(filled[1].1, root.path().join(pdfium));
    }

    #[test]
    fn a_full_package_fills_everything_and_an_empty_prefix_nothing() {
        let ort = format!("lib/{DLL_PREFIX}onnxruntime{DLL_SUFFIX}");
        let pdfium = format!("lib/{DLL_PREFIX}pdfium{DLL_SUFFIX}");
        let root = package(
            &[
                "share/lanrurugi/frontend",
                "share/lanrurugi/plugins",
                "share/lanrurugi/docs",
                "share/lanrurugi/models/manga-ocr",
                "share/lanrurugi/models/inpaint",
                "share/lanrurugi/models/bubble-seg",
            ],
            &[&ort, &pdfium],
        );
        assert_eq!(defaults_for(root.path(), |_| false).len(), entries().len());
        let empty = tempfile::tempdir().unwrap();
        assert!(defaults_for(empty.path(), |_| false).is_empty());
    }
}
