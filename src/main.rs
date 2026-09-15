//! Anna's Kobo: search shadow libraries, download with an Anna's Archive
//! membership key, convert EPUBs to KEPUB on the device, and hand the books
//! to Nickel. Runs as a small HTTP server that the Kobo's own browser opens
//! from a NickelMenu entry.

mod annas;
mod config;
mod jobs;
mod kepub;
mod libgen;
mod net;
mod nickel;
mod pages;
mod slum;
mod web;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEVICE_DATA_DIR: &str = "/mnt/onboard/.adds/annas-kobo";

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // The HTML/XML parsers inside kepub-rs and scraper are chatty.
        m.level() <= log::Level::Info
            && !m.target().starts_with("xml5ever")
            && !m.target().starts_with("html5ever")
            && !m.target().starts_with("markup5ever")
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let s = t % 86400;
        eprintln!(
            "{:02}:{:02}:{:02} {} {}",
            s / 3600,
            (s / 60) % 60,
            s % 60,
            r.level(),
            r.args()
        );
    }
    fn flush(&self) {}
}

fn usage() -> ! {
    eprintln!(
        "annas-kobo {VERSION}\n\
         usage:\n  \
           annas-kobo serve [--data-dir DIR] [--listen HOST:PORT]\n  \
           annas-kobo search [--source auto|annas|libgen] [--ext EXT] QUERY\n  \
           annas-kobo download MD5\n  \
           annas-kobo kepubify FILE...\n  \
           annas-kobo mirrors\n  \
           annas-kobo import"
    );
    std::process::exit(2)
}

fn default_data_dir() -> PathBuf {
    if Path::new(DEVICE_DATA_DIR).is_dir() {
        PathBuf::from(DEVICE_DATA_DIR)
    } else {
        PathBuf::from("data")
    }
}

fn main() {
    log::set_logger(&Logger).ok();
    log::set_max_level(log::LevelFilter::Info);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    let mut data_dir = default_data_dir();
    let mut listen: Option<String> = None;
    let mut source = "auto".to_string();
    let mut ext = String::new();
    let mut rest: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--data-dir" => {
                i += 1;
                data_dir = PathBuf::from(args.get(i).unwrap_or_else(|| usage()));
            }
            "--listen" => {
                i += 1;
                listen = Some(args.get(i).cloned().unwrap_or_else(|| usage()));
            }
            "--source" => {
                i += 1;
                source = args.get(i).cloned().unwrap_or_else(|| usage());
            }
            "--ext" => {
                i += 1;
                ext = args.get(i).cloned().unwrap_or_else(|| usage());
            }
            "-h" | "--help" => usage(),
            a => rest.push(a.to_string()),
        }
        i += 1;
    }

    std::fs::create_dir_all(&data_dir).ok();
    let cfg_path = data_dir.join("config.json");
    let mut cfg = config::load(&cfg_path);
    if let Some(l) = listen {
        cfg.listen = l;
    }
    let resolver = Arc::new(slum::Resolver::new());

    match cmd {
        "serve" => {
            log::info!("annas-kobo {VERSION} data dir {}", data_dir.display());
            web::serve(cfg, cfg_path, resolver);
        }
        "search" => {
            let q = rest.join(" ");
            if q.is_empty() {
                usage();
            }
            cfg.search_source = source;
            match web::run_search(&cfg, &resolver, &q, &ext) {
                Ok(out) => {
                    println!("source: {}  note: {}", out.source, out.note);
                    for b in out.books {
                        println!(
                            "{}  [{} {} {}]  {}  --  {}  ({})",
                            b.md5, b.format, b.size, b.language, b.title, b.authors, b.year
                        );
                    }
                }
                Err(e) => {
                    eprintln!("search failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        "download" => {
            let md5 = rest.first().cloned().unwrap_or_else(|| usage());
            let book = annas::Book {
                md5,
                title: "download".into(),
                ..Default::default()
            };
            cfg.deliver = "folder".into();
            let jobs = jobs::Jobs::new(Arc::new(std::sync::Mutex::new(cfg)), resolver, data_dir.join("cache"));
            let id = jobs.enqueue(book);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let snap = jobs.snapshot();
                let j = snap.iter().find(|j| j.id == id).unwrap();
                eprintln!("{:?} {} {}", j.state, j.bytes, j.detail);
                if matches!(j.state, jobs::State::Done | jobs::State::Failed) {
                    break;
                }
            }
        }
        "kepubify" => {
            if rest.is_empty() {
                usage();
            }
            for f in &rest {
                match kepub::kepubify(Path::new(f)) {
                    Ok(p) => println!("{f} -> {}", p.display()),
                    Err(e) => {
                        eprintln!("{f}: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        "mirrors" => match slum::fetch(net::agent()) {
            Ok(s) => {
                for m in &s.annas {
                    println!("annas   {:?}  {}", m.status, m.host);
                }
                for m in &s.libgen {
                    println!("libgen  {:?}  {}", m.status, m.host);
                }
                println!("chosen annas:  {}", resolver.annas_host(&cfg));
                println!("chosen libgen: {}", resolver.libgen_host(&cfg));
            }
            Err(e) => {
                eprintln!("slum: {e}");
                std::process::exit(1);
            }
        },
        "import" => {
            if let Err(e) = nickel::import() {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        _ => usage(),
    }
}
