use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Anna's Archive host to use, e.g. "annas-archive.gl". Empty = discover
    /// automatically from open-slum.org.
    pub base_url: String,
    /// Anna's Archive membership secret key (needed for downloads).
    pub secret_key: String,
    /// LibGen host override for the fallback search. Empty = automatic.
    pub libgen_url: String,
    /// "auto" (Anna's, then LibGen if Anna's is behind a browser check),
    /// "annas" or "libgen".
    pub search_source: String,
    /// "browser": the Kobo browser fetches the finished book from this app
    /// and Nickel adds it to the library at once (like the Google Drive
    /// integration). "folder": save into `download_dir` and import later
    /// with NickelMenu → Import books.
    pub deliver: String,
    /// Where downloaded books go in "folder" mode.
    pub download_dir: String,
    /// Convert downloaded EPUBs to KEPUB.
    pub kepubify: bool,
    /// Ask Nickel to import the new books when the download queue drains.
    pub auto_import: bool,
    /// Address the web UI listens on.
    pub listen: String,
    /// Results per search.
    pub max_results: usize,
}

impl Default for Config {
    fn default() -> Self {
        let on_device = Path::new("/mnt/onboard").is_dir();
        Self {
            base_url: String::new(),
            secret_key: String::new(),
            libgen_url: String::new(),
            search_source: "auto".into(),
            deliver: "browser".into(),
            download_dir: if on_device {
                "/mnt/onboard/Books".into()
            } else {
                "data/books".into()
            },
            kepubify: true,
            auto_import: true,
            listen: "0.0.0.0:8484".into(),
            max_results: 20,
        }
    }
}

pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Config>(&s) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config {} unreadable ({e}), using defaults", path.display());
                Config::default()
            }
        },
        Err(_) => {
            let c = Config::default();
            if let Err(e) = save(path, &c) {
                log::warn!("cannot write default config: {e}");
            }
            c
        }
    }
}

pub fn save(path: &Path, cfg: &Config) -> Result<(), String> {
    let s = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// "https://annas-archive.gl/" -> "annas-archive.gl"
pub fn normalize_host(raw: &str) -> String {
    let v = raw.trim();
    let v = v.strip_prefix("https://").or_else(|| v.strip_prefix("http://")).unwrap_or(v);
    v.trim_end_matches('/').to_string()
}
