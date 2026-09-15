//! EPUB -> KEPUB conversion, in-process (kepub-rs, a port of kepubify's
//! transform). Kobo's reader gets faster page turns, reading stats and
//! proper highlighting from the koboSpan markup.

use std::fs::File;
use std::path::{Path, PathBuf};

pub fn is_kepub(p: &Path) -> bool {
    p.to_string_lossy().to_lowercase().ends_with(".kepub.epub")
}

/// Converts `src` next to itself as `<name>.kepub.epub` and removes the
/// original on success.
pub fn kepubify(src: &Path) -> Result<PathBuf, String> {
    if is_kepub(src) {
        return Ok(src.to_path_buf());
    }
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or("no file name")?;
    let stem = name
        .strip_suffix(".epub")
        .or_else(|| name.strip_suffix(".EPUB"))
        .ok_or("not an .epub file")?;
    let dir = src.parent().unwrap_or(Path::new("."));
    let out = dir.join(format!("{stem}.kepub.epub"));
    let tmp = dir.join(format!("{stem}.kepub.epub.part"));
    let input = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let output = File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    let res = kepub_rs::Converter::default().convert(input, output);
    if let Err(e) = res {
        std::fs::remove_file(&tmp).ok();
        return Err(format!("kepub conversion failed: {e}"));
    }
    std::fs::rename(&tmp, &out).map_err(|e| format!("rename: {e}"))?;
    std::fs::remove_file(src).ok();
    Ok(out)
}
