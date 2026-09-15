//! The HTTP server the Kobo browser talks to.

use crate::annas::{self, Book, SearchError};
use crate::config::{self, Config};
use crate::jobs::Jobs;
use crate::libgen;
use crate::nickel;
use crate::pages;
use crate::slum::Resolver;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

#[derive(Clone)]
pub struct SearchOutcome {
    pub books: Vec<Book>,
    pub source: String,
    pub note: String,
}

enum SearchEntry {
    Running(Instant),
    Done(Instant, Result<SearchOutcome, String>),
}

/// Anna's first (unless configured otherwise); LibGen when Anna's search
/// pages sit behind the DDoS-Guard browser check, which a device without a
/// JavaScript client cannot pass. Downloads always go through Anna's API.
pub fn run_search(cfg: &Config, resolver: &Resolver, q: &str, ext: &str) -> Result<SearchOutcome, String> {
    let max = cfg.max_results.max(1);
    let mut note = String::new();
    if cfg.search_source != "libgen" {
        match resolver.annas_blocked_for() {
            Some(mins) if cfg.search_source != "annas" => {
                note = format!(
                    "Anna's Archive search is behind a browser check (seen {mins} min ago), so these come from LibGen. Same files, same md5s."
                );
            }
            _ => {
                let host = resolver.annas_host(cfg);
                match annas::search(&host, q, ext, max) {
                    Ok(books) => {
                        return Ok(SearchOutcome { books, source: format!("Anna's Archive ({host})"), note });
                    }
                    Err(SearchError::Blocked(h)) => {
                        resolver.mark_annas_blocked();
                        if cfg.search_source == "annas" {
                            return Err(format!(
                                "{h} shows a browser check on search pages; switch Search source to auto or LibGen in Settings"
                            ));
                        }
                        note = format!("{h} needs a browser check for its search pages right now, so these come from LibGen. Same files, same md5s.");
                    }
                    Err(SearchError::Failed(e)) => {
                        resolver.forget_annas();
                        if cfg.search_source == "annas" {
                            return Err(e);
                        }
                        note = format!("Anna's Archive search failed ({e}); showing LibGen results.");
                    }
                }
            }
        }
    }
    let mut last = String::from("no LibGen mirror reachable");
    for host in resolver.libgen_candidates(cfg).into_iter().take(4) {
        match libgen::search(&host, q, ext, max) {
            Ok(books) => {
                resolver.set_libgen(&host);
                return Ok(SearchOutcome { books, source: format!("LibGen ({host})"), note });
            }
            Err(e) => {
                log::warn!("libgen {host}: {e}");
                last = format!("{host}: {e}");
            }
        }
    }
    resolver.forget_libgen();
    Err(if note.is_empty() { last } else { format!("{note} LibGen failed too: {last}") })
}

struct App {
    cfg: Arc<Mutex<Config>>,
    cfg_path: PathBuf,
    resolver: Arc<Resolver>,
    jobs: Arc<Jobs>,
    /// Books from recent searches, so the download form only needs the md5.
    seen: Mutex<HashMap<String, Book>>,
    /// Searches run in the background; the page polls until one is done,
    /// so the Kobo's browser has something to show meanwhile.
    searches: Mutex<HashMap<String, SearchEntry>>,
}

fn query(url: &str) -> HashMap<String, String> {
    let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    form_urlencoded::parse(q.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn form(req: &mut Request) -> HashMap<String, String> {
    let mut body = String::new();
    req.as_reader().take(1 << 20).read_to_string(&mut body).ok();
    form_urlencoded::parse(body.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn html(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn redirect(to: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string("")
        .with_status_code(303)
        .with_header(Header::from_bytes(&b"Location"[..], to.as_bytes()).unwrap())
}

impl App {
    fn search_page(self: &Arc<Self>, term: &str, ext: &str, active: usize) -> Response<std::io::Cursor<Vec<u8>>> {
        let key = format!("{term}\u{0}{ext}");
        let mut searches = self.searches.lock().unwrap();
        // Drop stale results so the map stays small.
        searches.retain(|_, e| match e {
            SearchEntry::Running(t) => t.elapsed() < Duration::from_secs(300),
            SearchEntry::Done(t, _) => t.elapsed() < Duration::from_secs(600),
        });
        match searches.get(&key) {
            Some(SearchEntry::Running(t)) => {
                let secs = t.elapsed().as_secs();
                let (annas, libgen) = self.resolver.current();
                return html(pages::searching(term, ext, secs, annas.is_some() || libgen.is_some(), active));
            }
            Some(SearchEntry::Done(_, Ok(out))) => {
                let out = out.clone();
                let mut seen = self.seen.lock().unwrap();
                for b in &out.books {
                    seen.insert(b.md5.clone(), b.clone());
                }
                return html(pages::results(term, ext, &out.source, &out.note, &out.books, active));
            }
            Some(SearchEntry::Done(_, Err(e))) => {
                let e = e.clone();
                searches.remove(&key);
                return html(pages::error("Search failed", &e, active));
            }
            None => {}
        }
        searches.insert(key.clone(), SearchEntry::Running(Instant::now()));
        drop(searches);
        let app = self.clone();
        let (term_s, ext_s) = (term.to_string(), ext.to_string());
        std::thread::spawn(move || {
            let cfg = app.cfg.lock().unwrap().clone();
            let started = Instant::now();
            let r = run_search(&cfg, &app.resolver, &term_s, &ext_s);
            log::info!("search {term_s:?} took {} ms", started.elapsed().as_millis());
            app.searches.lock().unwrap().insert(key, SearchEntry::Done(Instant::now(), r));
        });
        html(pages::searching(term, ext, 0, false, active))
    }

    fn handle(self: &Arc<Self>, mut req: Request) {
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();
        let method = req.method().clone();
        let active = self.jobs.active_count();
        let resp = match (method, path.as_str()) {
            (Method::Get, "/health") => Response::from_string("ok"),
            // Read-only diagnostics for when ssh is not available.
            (Method::Get, "/log") => {
                let dir = self.cfg_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
                let text = std::fs::read_to_string(dir.join("annas-kobo.log")).unwrap_or_default();
                let tail: Vec<&str> = text.lines().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
                Response::from_string(tail.join("\n"))
            }
            (Method::Get, "/syslog") => {
                let q = query(&url);
                let n = q.get("n").and_then(|v| v.parse::<usize>().ok()).unwrap_or(300);
                let out = std::process::Command::new("logread").output();
                let text = match out {
                    Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                    Err(e) => format!("logread: {e}"),
                };
                let tail: Vec<&str> = text.lines().rev().take(n).collect::<Vec<_>>().into_iter().rev().collect();
                Response::from_string(tail.join("\n"))
            }
            (Method::Get, "/") => {
                let cfg = self.cfg.lock().unwrap().clone();
                let (a, l) = self.resolver.current();
                let msg = query(&url).remove("msg").unwrap_or_default();
                html(pages::home(&cfg, active, a, l, &msg))
            }
            (Method::Get, "/search") => {
                let q = query(&url);
                let term = q.get("q").map(|s| s.trim().to_string()).unwrap_or_default();
                let ext = q.get("ext").cloned().unwrap_or_default();
                if term.is_empty() {
                    redirect("/")
                } else {
                    self.search_page(&term, &ext, active)
                }
            }
            (Method::Post, "/download") => {
                let f = form(&mut req);
                let md5 = f.get("md5").cloned().unwrap_or_default();
                let book = self.seen.lock().unwrap().get(&md5).cloned();
                match book {
                    Some(b) => {
                        self.jobs.enqueue(b);
                        redirect("/jobs")
                    }
                    None if annas::is_md5(&md5) => {
                        self.jobs.enqueue(Book { md5: md5.clone(), title: md5, ..Default::default() });
                        redirect("/jobs")
                    }
                    None => html(pages::error("Download", "unknown book; search again", active)),
                }
            }
            (Method::Get, "/jobs") => {
                let q = query(&url);
                let cfg = self.cfg.lock().unwrap().clone();
                let mut msg = String::new();
                if q.contains_key("import") {
                    msg = "Asked Nickel to rescan the library.".into();
                }
                if let Some(Err(e)) = self.jobs.last_import.lock().unwrap().as_ref() {
                    msg = format!("Last import attempt failed: {e}");
                }
                // Browser delivery: the first finished book that has not been
                // fetched yet is pushed to the browser as a download.
                let deliver = if cfg.deliver != "folder" { self.jobs.take_undelivered().map(|j| j.id) } else { None };
                html(pages::jobs(
                    &self.jobs.snapshot(),
                    active,
                    &msg,
                    nickel::available(),
                    nickel::on_kobo(),
                    cfg.deliver != "folder",
                    deliver,
                ))
            }
            (Method::Get, p) if p.starts_with("/file/") => {
                let id: u64 = p.trim_start_matches("/file/").split('/').next().unwrap_or("").parse().unwrap_or(0);
                match self.jobs.get(id).and_then(|j| j.path) {
                    Some(path) => match std::fs::File::open(&path) {
                        Ok(f) => {
                            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let len = f.metadata().map(|m| m.len() as usize).ok();
                            let mime = if name.to_lowercase().ends_with(".pdf") {
                                "application/pdf"
                            } else if name.to_lowercase().ends_with(".epub") {
                                "application/epub+zip"
                            } else {
                                "application/octet-stream"
                            };
                            let ascii = crate::jobs::ascii_name(&name);
                            let disp = format!(
                                "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
                                form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+', "%20")
                            );
                            let resp = Response::new(tiny_http::StatusCode(200), Vec::new(), f, len, None)
                                .with_header(Header::from_bytes(&b"Content-Type"[..], mime.as_bytes()).unwrap())
                                .with_header(Header::from_bytes(&b"Content-Disposition"[..], disp.as_bytes()).unwrap());
                            log::info!("serving {} to the browser", path.display());
                            let _ = req.respond(resp);
                            return;
                        }
                        Err(e) => html(pages::error("File", &format!("cannot open {}: {e}", path.display()), active)),
                    },
                    None => html(pages::error("File", "no such download", active)),
                }
            }
            (Method::Post, "/jobs/retry") => {
                let f = form(&mut req);
                let id: u64 = f.get("id").and_then(|v| v.parse().ok()).unwrap_or(0);
                self.jobs.retry(id);
                redirect("/jobs")
            }
            (Method::Post, "/jobs/clear") => {
                self.jobs.clear_finished();
                redirect("/jobs")
            }
            (Method::Post, "/import") => {
                let jobs = self.jobs.clone();
                std::thread::spawn(move || {
                    let r = nickel::import().map(|_| std::time::SystemTime::now());
                    *jobs.last_import.lock().unwrap() = Some(r);
                });
                redirect("/jobs?import=1")
            }
            (Method::Get, "/settings") => {
                let cfg = self.cfg.lock().unwrap().clone();
                let saved = query(&url).contains_key("saved");
                html(pages::settings(&cfg, saved, active, &self.cfg_path.parent().unwrap_or(std::path::Path::new(".")).display().to_string()))
            }
            (Method::Post, "/settings") => {
                let f = form(&mut req);
                let mut cfg = self.cfg.lock().unwrap();
                let old_base = cfg.base_url.clone();
                let old_lg = cfg.libgen_url.clone();
                let get = |k: &str| f.get(k).map(|s| s.trim().to_string()).unwrap_or_default();
                cfg.secret_key = get("secret_key");
                cfg.base_url = config::normalize_host(&get("base_url"));
                cfg.libgen_url = config::normalize_host(&get("libgen_url"));
                let src = get("search_source");
                cfg.search_source = if ["auto", "annas", "libgen"].contains(&src.as_str()) { src } else { "auto".into() };
                let dir = get("download_dir");
                if !dir.is_empty() {
                    cfg.download_dir = dir;
                }
                if let Ok(n) = get("max_results").parse::<usize>() {
                    cfg.max_results = n.clamp(1, 100);
                }
                cfg.deliver = if get("deliver") == "folder" { "folder".into() } else { "browser".into() };
                cfg.kepubify = f.contains_key("kepubify");
                cfg.auto_import = f.contains_key("auto_import");
                if cfg.base_url != old_base {
                    self.resolver.forget_annas();
                }
                if cfg.libgen_url != old_lg {
                    self.resolver.forget_libgen();
                }
                let r = config::save(&self.cfg_path, &cfg);
                drop(cfg);
                match r {
                    Ok(()) => redirect("/settings?saved=1"),
                    Err(e) => html(pages::error("Settings", &format!("could not save: {e}"), active)),
                }
            }
            (Method::Post, "/quit") => {
                log::info!("quit requested from the UI");
                let _ = req.respond(html(pages::error(
                    "Anna's Kobo",
                    "Background service stopped. Close this window; the NickelMenu entry starts it again.",
                    0,
                )));
                std::thread::sleep(std::time::Duration::from_millis(300));
                std::process::exit(0);
            }
            _ => Response::from_string("not found").with_status_code(404),
        };
        let _ = req.respond(resp);
    }
}

pub fn serve(cfg: Config, cfg_path: PathBuf, resolver: Arc<Resolver>) {
    let listen = cfg.listen.clone();
    let cfg = Arc::new(Mutex::new(cfg));
    // Nickel scans all of /mnt/onboard, dot-folders included, so books
    // waiting for the browser live in RAM; the browser's own copy is the one
    // that ends up in the library.
    let cache_dir = if nickel::on_kobo() {
        PathBuf::from("/tmp/annas-kobo")
    } else {
        cfg_path.parent().unwrap_or(std::path::Path::new(".")).join("cache")
    };
    let jobs = Jobs::new(cfg.clone(), resolver.clone(), cache_dir);
    let app = Arc::new(App {
        cfg,
        cfg_path,
        resolver: resolver.clone(),
        jobs,
        seen: Mutex::new(HashMap::new()),
        searches: Mutex::new(HashMap::new()),
    });
    // Fetch the mirror list now so the first search does not pay for it.
    let warm_cfg = app.cfg.lock().unwrap().clone();
    std::thread::spawn(move || {
        resolver.warm();
        if !warm_cfg.secret_key.trim().is_empty() {
            resolver.annas_host(&warm_cfg);
        }
    });
    let server = match Server::http(&listen) {
        Ok(s) => s,
        Err(e) => {
            log::error!("cannot listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    log::info!("listening on http://{listen}/");
    for req in server.incoming_requests() {
        let app = app.clone();
        std::thread::spawn(move || app.handle(req));
    }
}
