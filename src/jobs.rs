//! Download queue: one worker thread, fast-download URL via Anna's API,
//! stream to disk, convert, then ask Nickel to import once the queue is
//! empty.

use crate::annas::{self, Book};
use crate::config::Config;
use crate::kepub;
use crate::net;
use crate::nickel;
use crate::slum::Resolver;
use std::io::{Read, Write};
use ureq::ResponseExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Queued,
    Resolving,
    Downloading,
    Converting,
    Done,
    Failed,
}

impl State {
    pub fn active(self) -> bool {
        matches!(self, State::Queued | State::Resolving | State::Downloading | State::Converting)
    }
    pub fn label(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Resolving => "finding a download link",
            State::Downloading => "downloading",
            State::Converting => "converting to kepub",
            State::Done => "done",
            State::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Job {
    pub id: u64,
    pub book: Book,
    pub state: State,
    pub detail: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub path: Option<PathBuf>,
    pub created: SystemTime,
    /// Handed to the Kobo browser already (browser delivery).
    pub delivered: bool,
}

pub struct Jobs {
    /// Books waiting for the browser to fetch them (browser delivery).
    pub cache_dir: PathBuf,
    list: Mutex<Vec<Job>>,
    tx: Mutex<Sender<u64>>,
    cfg: Arc<Mutex<Config>>,
    resolver: Arc<Resolver>,
    next_id: AtomicU64,
    import_pending: AtomicBool,
    pub last_import: Mutex<Option<Result<SystemTime, String>>>,
}

impl Jobs {
    pub fn new(cfg: Arc<Mutex<Config>>, resolver: Arc<Resolver>, cache_dir: PathBuf) -> Arc<Jobs> {
        let (tx, rx) = channel::<u64>();
        let jobs = Arc::new(Jobs {
            cache_dir,
            list: Mutex::new(Vec::new()),
            tx: Mutex::new(tx),
            cfg,
            resolver,
            next_id: AtomicU64::new(1),
            import_pending: AtomicBool::new(false),
            last_import: Mutex::new(None),
        });
        let worker = jobs.clone();
        std::thread::Builder::new()
            .name("downloads".into())
            .spawn(move || {
                while let Ok(id) = rx.recv() {
                    worker.run(id);
                    worker.maybe_import();
                }
            })
            .expect("spawn worker");
        jobs
    }

    pub fn enqueue(&self, book: Book) -> u64 {
        let mut list = self.list.lock().unwrap();
        if let Some(j) = list.iter().find(|j| j.book.md5 == book.md5 && j.state.active()) {
            return j.id;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        list.push(Job {
            id,
            book,
            state: State::Queued,
            detail: String::new(),
            bytes: 0,
            total: None,
            path: None,
            created: SystemTime::now(),
            delivered: false,
        });
        drop(list);
        self.tx.lock().unwrap().send(id).ok();
        id
    }

    pub fn snapshot(&self) -> Vec<Job> {
        let mut v = self.list.lock().unwrap().clone();
        v.reverse();
        v
    }

    pub fn active_count(&self) -> usize {
        self.list.lock().unwrap().iter().filter(|j| j.state.active()).count()
    }

    pub fn clear_finished(&self) {
        let mut list = self.list.lock().unwrap();
        for j in list.iter().filter(|j| !j.state.active()) {
            if let Some(p) = &j.path {
                if p.starts_with(&self.cache_dir) {
                    std::fs::remove_file(p).ok();
                }
            }
        }
        list.retain(|j| j.state.active());
    }

    /// Puts a failed job back in the queue.
    pub fn retry(&self, id: u64) -> bool {
        let mut list = self.list.lock().unwrap();
        let Some(j) = list.iter_mut().find(|j| j.id == id && j.state == State::Failed) else {
            return false;
        };
        j.state = State::Queued;
        j.detail.clear();
        j.bytes = 0;
        j.total = None;
        j.path = None;
        j.delivered = false;
        drop(list);
        self.tx.lock().unwrap().send(id).ok();
        true
    }

    pub fn get(&self, id: u64) -> Option<Job> {
        self.list.lock().unwrap().iter().find(|j| j.id == id).cloned()
    }

    /// The next finished book the browser has not fetched yet, marked as
    /// handed over.
    pub fn take_undelivered(&self) -> Option<Job> {
        let mut list = self.list.lock().unwrap();
        let j = list.iter_mut().find(|j| j.state == State::Done && !j.delivered && j.path.is_some())?;
        j.delivered = true;
        Some(j.clone())
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Job)) {
        let mut list = self.list.lock().unwrap();
        if let Some(j) = list.iter_mut().find(|j| j.id == id) {
            f(j);
        }
    }

    fn run(&self, id: u64) {
        let Some(job) = self.list.lock().unwrap().iter().find(|j| j.id == id).cloned() else {
            return;
        };
        let cfg = self.cfg.lock().unwrap().clone();
        match self.download(id, &job.book, &cfg) {
            Ok(path) => {
                log::info!("job {id} done: {}", path.display());
                self.update(id, |j| {
                    j.state = State::Done;
                    j.path = Some(path);
                    j.detail.clear();
                });
                self.import_pending.store(true, Ordering::SeqCst);
                self.purge_cache();
            }
            Err(e) => {
                log::warn!("job {id} failed: {e}");
                self.update(id, |j| {
                    j.state = State::Failed;
                    j.detail = e;
                });
            }
        }
    }

    fn download(&self, id: u64, book: &Book, cfg: &Config) -> Result<PathBuf, String> {
        self.update(id, |j| j.state = State::Resolving);
        // Anna's fast servers with a membership key; otherwise (or if that
        // fails) LibGen's free servers, which hold the same files. Mirrors
        // and their CDNs fail often (HTTP 500), so every step walks on to
        // the next candidate.
        let mut resp = None;
        let mut last_err = String::from("no download source reachable");
        if !cfg.secret_key.trim().is_empty() {
            let host = self.resolver.annas_host(cfg);
            match annas::fast_download_url(&host, &book.md5, &cfg.secret_key)
                .and_then(|u| open_stream(&u))
            {
                Ok(r) => {
                    log::info!("job {id} via Anna's Archive");
                    resp = Some(r);
                }
                Err(e) => {
                    log::warn!("job {id}: Anna's fast download failed ({e}), trying LibGen");
                    if e.contains("not the API JSON") || e.contains("io:") {
                        self.resolver.forget_annas();
                    }
                    self.update(id, |j| j.detail = format!("Anna's: {e}; using LibGen"));
                    last_err = e;
                }
            }
        }
        if resp.is_none() {
            // Three rounds over the mirror list, with a pause in between:
            // their 500s are usually momentary.
            let hosts: Vec<String> = self.resolver.libgen_candidates(cfg).into_iter().take(5).collect();
            let mut errors: Vec<String> = Vec::new();
            'rounds: for round in 0..3 {
                if round > 0 {
                    self.update(id, |j| j.detail = format!("mirrors busy, retrying (round {})", round + 1));
                    std::thread::sleep(std::time::Duration::from_secs(4));
                }
                for host in &hosts {
                    match crate::libgen::download_url(host, &book.md5).and_then(|u| open_stream(&u)) {
                        Ok(r) => {
                            log::info!("job {id} via LibGen ({host})");
                            resp = Some(r);
                            self.update(id, |j| j.detail.clear());
                            break 'rounds;
                        }
                        Err(e) => {
                            log::warn!("job {id}: {host}: {e}");
                            if e.contains("no download link") {
                                return Err(e);
                            }
                            let short = e.replace(" for the download page", "").replace("download server answered ", "");
                            let entry = format!("{host}: {short}");
                            if !errors.contains(&entry) {
                                errors.push(entry);
                            }
                        }
                    }
                }
            }
            if resp.is_none() {
                last_err = format!("every mirror failed ({}). Tap Retry in a minute.", errors.join("; "));
            }
        }
        let Some(mut resp) = resp else {
            self.resolver.forget_libgen();
            return Err(last_err);
        };
        self.update(id, |j| j.state = State::Downloading);
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let disposition = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        self.update(id, |j| j.total = total);

        let url = resp.get_uri().to_string();
        let ext = pick_ext(book, &url, disposition.as_deref());
        let dir = if cfg.deliver == "folder" {
            PathBuf::from(&cfg.download_dir)
        } else {
            self.cache_dir.clone()
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let base = file_base(book);
        let mut final_path = dir.join(format!("{base}.{ext}"));
        let mut n = 2;
        while final_path.exists() || kepub_twin(&final_path).exists() {
            final_path = dir.join(format!("{base} ({n}).{ext}"));
            n += 1;
        }
        let part = final_path.with_extension(format!("{ext}.part"));

        let mut out = std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
        let mut reader = resp.body_mut().with_config().limit(2 * 1024 * 1024 * 1024).reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut written: u64 = 0;
        let mut since_update: u64 = 0;
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                std::fs::remove_file(&part).ok();
                format!("download interrupted after {} bytes: {e}", written)
            })?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| {
                std::fs::remove_file(&part).ok();
                format!("write: {e}")
            })?;
            written += n as u64;
            since_update += n as u64;
            if since_update >= 256 * 1024 {
                since_update = 0;
                self.update(id, |j| j.bytes = written);
            }
        }
        out.flush().ok();
        drop(out);
        self.update(id, |j| j.bytes = written);
        if let Some(t) = total {
            if written < t {
                std::fs::remove_file(&part).ok();
                return Err(format!("short download: {written} of {t} bytes"));
            }
        }
        if written < 1024 {
            let peek = std::fs::read(&part).unwrap_or_default();
            std::fs::remove_file(&part).ok();
            return Err(format!(
                "server sent {written} bytes instead of a book: {}",
                String::from_utf8_lossy(&peek[..peek.len().min(200)])
            ));
        }
        std::fs::rename(&part, &final_path).map_err(|e| format!("rename: {e}"))?;

        if ext == "epub" && cfg.kepubify {
            self.update(id, |j| j.state = State::Converting);
            match kepub::kepubify(&final_path) {
                Ok(p) => return Ok(p),
                Err(e) => {
                    // Keep the plain EPUB; Kobo reads it too, just slower.
                    log::warn!("job {id}: {e}; keeping plain epub");
                    self.update(id, |j| j.detail = format!("kept as plain EPUB: {e}"));
                }
            }
        }
        Ok(final_path)
    }

    /// Drops cached books older than two hours (RAM on the device).
    fn purge_cache(&self) {
        let Ok(rd) = std::fs::read_dir(&self.cache_dir) else { return };
        for e in rd.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().map(|d| d.as_secs() > 2 * 3600).unwrap_or(false))
                .unwrap_or(false);
            if old {
                std::fs::remove_file(e.path()).ok();
            }
        }
    }

    fn maybe_import(&self) {
        if self.active_count() > 0 || !self.import_pending.load(Ordering::SeqCst) {
            return;
        }
        let cfg = self.cfg.lock().unwrap().clone();
        if cfg.deliver != "folder" || !cfg.auto_import || !nickel::available() {
            return;
        }
        self.import_pending.store(false, Ordering::SeqCst);
        let r = nickel::import().map(|_| SystemTime::now());
        *self.last_import.lock().unwrap() = Some(r);
    }
}

/// Starts a GET and checks the status, so a dead CDN fails before any
/// bytes are written.
fn open_stream(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    log::info!("fetching {url}");
    let resp = net::download_agent().get(url).call().map_err(|e| format!("download: {e}"))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("download server answered HTTP {status}"));
    }
    Ok(resp)
}

fn kepub_twin(p: &PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_suffix(".epub") {
        Some(stem) => PathBuf::from(format!("{stem}.kepub.epub")),
        None => p.clone(),
    }
}

fn pick_ext(book: &Book, url: &str, disposition: Option<&str>) -> String {
    let known = ["epub", "pdf", "mobi", "azw3", "azw", "djvu", "cbz", "cbr", "fb2", "txt", "doc", "docx", "rtf", "lit"];
    let f = book.format.to_lowercase();
    if known.contains(&f.as_str()) {
        return f;
    }
    let from = |s: &str| -> Option<String> {
        let s = s.split(['?', '#']).next().unwrap_or(s);
        let e = s.rsplit('.').next()?.trim_matches('"').to_lowercase();
        known.contains(&e.as_str()).then_some(e)
    };
    disposition
        .and_then(from)
        .or_else(|| from(url))
        .unwrap_or_else(|| "epub".into())
}

/// The Kobo browser names its download from the plain `filename=` value,
/// which must be ASCII; map common Latin letters instead of losing them.
pub fn ascii_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii() && c != '"' && c != '\\' {
            out.push(c);
            continue;
        }
        let rep = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
            'ç' | 'ć' | 'č' => "c",
            'Ç' | 'Ć' | 'Č' => "C",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
            'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ė' | 'Ę' | 'Ě' => "E",
            'ğ' | 'ģ' => "g",
            'Ğ' | 'Ģ' => "G",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' | 'į' => "i",
            'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'İ' | 'Į' => "I",
            'ł' | 'ļ' => "l",
            'Ł' | 'Ļ' => "L",
            'ñ' | 'ń' | 'ň' | 'ņ' => "n",
            'Ñ' | 'Ń' | 'Ň' | 'Ņ' => "N",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
            'ř' => "r",
            'Ř' => "R",
            'ś' | 'š' | 'ş' | 'ș' => "s",
            'Ś' | 'Š' | 'Ş' | 'Ș' => "S",
            'ť' | 'ţ' | 'ț' => "t",
            'Ť' | 'Ţ' | 'Ț' => "T",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' | 'Ų' => "U",
            'ý' | 'ÿ' => "y",
            'Ý' | 'Ÿ' => "Y",
            'ź' | 'ž' | 'ż' => "z",
            'Ź' | 'Ž' | 'Ż' => "Z",
            'ß' => "ss",
            'æ' => "ae",
            'Æ' => "AE",
            'œ' => "oe",
            'Œ' => "OE",
            '‘' | '’' => "'",
            '“' | '”' => "",
            '–' | '—' => "-",
            _ => "_",
        };
        out.push_str(rep);
    }
    out
}

/// FAT-safe, short, readable: "Title - Author".
pub fn file_base(book: &Book) -> String {
    let safe = |s: &str| -> String {
        let mut out: String = s
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
                c if (c as u32) < 32 => ' ',
                c => c,
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out = out.trim_matches(|c| c == '.' || c == ' ').to_string();
        out
    };
    let mut title = safe(&book.title);
    if title.chars().count() > 80 {
        title = title.chars().take(80).collect::<String>().trim_end().to_string();
    }
    if title.is_empty() {
        title = book.md5.clone();
    }
    let mut authors = safe(&book.authors);
    if authors.chars().count() > 40 {
        authors = authors.chars().take(40).collect::<String>().trim_end().to_string();
    }
    if authors.is_empty() {
        title
    } else {
        format!("{title} - {authors}")
    }
}
