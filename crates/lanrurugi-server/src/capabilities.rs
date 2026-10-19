//! One start-up summary of the optional native components, so a native install says up front which
//! features it cannot offer instead of each one failing later. Presence checks only: nothing is
//! loaded into this process.

use std::env::consts::{DLL_PREFIX, DLL_SUFFIX};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directories the dynamic loader searches by default on the supported systems.
const LIBRARY_DIRS: &[&str] = &[
    "/usr/local/lib",
    "/usr/lib",
    "/usr/lib64",
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/opt/homebrew/lib",
];

/// `explicit` if it exists, else the first `name` in the loader's search directories.
fn find_library(explicit: Option<OsString>, name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(path) = explicit {
        let path = PathBuf::from(path);
        // A bare file name is handed to the loader as-is; it is only checkable by searching.
        if path.components().count() > 1 {
            return path.exists().then_some(path);
        }
    }
    extra_dirs
        .iter()
        .cloned()
        .chain(LIBRARY_DIRS.iter().map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|path| path.exists())
}

/// `program` as given if it is a path, else its first match on `PATH`.
fn find_program(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return program.exists().then(|| program.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

fn loader_path_dirs() -> Vec<PathBuf> {
    ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"]
        .iter()
        .filter_map(std::env::var_os)
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .collect()
}

/// Logs what is available, and one warning naming every feature that is not.
pub fn log_summary(deno_bin: &str) {
    let extra = loader_path_dirs();
    let pdfium = find_library(
        std::env::var_os("LANRURUGI_PDFIUM_LIB"),
        &format!("{DLL_PREFIX}pdfium{DLL_SUFFIX}"),
        &extra,
    );
    let ort = find_library(
        std::env::var_os("ORT_DYLIB_PATH"),
        &format!("{DLL_PREFIX}onnxruntime{DLL_SUFFIX}"),
        &extra,
    );
    let djvu_tool = match std::env::var_os("LANRURUGI_DJVU_BIN_DIR") {
        Some(dir) => PathBuf::from(dir).join("ddjvu"),
        None => PathBuf::from("ddjvu"),
    };
    let djvu = find_program(&djvu_tool);
    let deno = find_program(Path::new(deno_bin));
    let ocr_model = lanrurugi_ocr::find_model_dir().ok();
    let inpaint_model = lanrurugi_inpaint::model_discovery::find_model_dir().ok();
    let bubble_model = lanrurugi_ocr::bubble_segment::model_discovery::find_model_dir().ok();

    let show = |found: &Option<PathBuf>| match found {
        Some(path) => path.display().to_string(),
        None => "not found".to_owned(),
    };
    tracing::info!(
        pdfium = %show(&pdfium),
        onnxruntime = %show(&ort),
        djvulibre = %show(&djvu),
        deno = %show(&deno),
        ocr_model = %show(&ocr_model),
        inpaint_model = %show(&inpaint_model),
        bubble_model = %show(&bubble_model),
        "optional components"
    );

    let mut unavailable = Vec::new();
    if pdfium.is_none() {
        unavailable.push("PDF books (PDFium)");
    }
    if djvu.is_none() {
        unavailable.push("DjVu books (DjVuLibre)");
    }
    if deno.is_none() {
        unavailable.push("plugins, downloads and subscriptions (Deno)");
    }
    if ort.is_none() {
        unavailable.push("recommendations, OCR and translation (ONNX Runtime)");
    } else if ocr_model.is_none() || inpaint_model.is_none() {
        unavailable.push("translation (models not downloaded — run fetch-models.sh)");
    }
    if !unavailable.is_empty() {
        tracing::warn!(
            features = %unavailable.join("; "),
            "some features are unavailable on this system"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_library_paths_are_checked_and_bare_names_searched() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("libdemo.so");
        std::fs::write(&lib, b"").unwrap();
        let extra = [dir.path().to_path_buf()];
        assert_eq!(
            find_library(Some(lib.clone().into()), "x", &[]),
            Some(lib.clone())
        );
        assert_eq!(
            find_library(
                Some(dir.path().join("nope.so").into()),
                "libdemo.so",
                &extra
            ),
            None
        );
        assert_eq!(
            find_library(Some("libdemo.so".into()), "libdemo.so", &extra),
            Some(lib.clone())
        );
        assert_eq!(find_library(None, "libdemo.so", &extra), Some(lib));
        assert_eq!(
            find_library(None, "libnot-installed-anywhere.so", &[]),
            None
        );
    }

    #[test]
    fn programs_are_found_on_path_or_by_path() {
        assert!(find_program(Path::new("sh")).is_some());
        assert!(find_program(Path::new("/bin/sh")).is_some());
        assert!(find_program(Path::new("definitely-not-a-program-here")).is_none());
    }
}
