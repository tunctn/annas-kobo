//! Server-rendered HTML for the Kobo's browser: plain forms, big targets,
//! black on white, no JavaScript needed (meta refresh for progress).

use crate::annas::Book;
use crate::config::Config;
use crate::jobs::{Job, State};
use html_escape::encode_text as esc;

const CSS: &str = r#"
body{font-family:Georgia,serif;font-size:21px;line-height:1.35;margin:0;padding:0 14px 30px;background:#fff;color:#000}
.top{border-bottom:3px solid #000;padding:12px 0 10px;margin-bottom:14px}
.top a{display:inline-block;padding:10px 14px;margin:0 8px 6px 0;border:2px solid #000;text-decoration:none;color:#000;font-weight:bold;font-family:Helvetica,Arial,sans-serif}
.top a.brand{border:none;padding-left:0;font-size:24px}
h1,h2{font-family:Helvetica,Arial,sans-serif;margin:8px 0 12px}
h2{font-size:22px}
input[type=text],input[type=password],select{font-size:22px;padding:10px;border:2px solid #000;width:100%;box-sizing:border-box;background:#fff;color:#000;margin:4px 0 12px}
button,.btn{font-size:21px;padding:12px 18px;border:3px solid #000;background:#fff;color:#000;font-weight:bold;font-family:Helvetica,Arial,sans-serif;text-decoration:none;display:inline-block}
button.small{font-size:18px;padding:8px 12px;border-width:2px}
.book{border-bottom:2px solid #000;padding:14px 0}
.title{font-size:22px;font-weight:bold}
.meta{margin:6px 0 8px}
.tag{display:inline-block;border:1px solid #000;padding:1px 7px;margin:2px 6px 2px 0;font-family:Helvetica,Arial,sans-serif;font-size:18px}
.tag.b{background:#000;color:#fff}
.box{border:3px solid #000;padding:12px 14px;margin:12px 0}
.muted{font-size:18px}
.bar{border:2px solid #000;height:18px;margin:6px 0;position:relative}
.bar div{background:#000;height:18px}
label{display:block;margin-top:10px;font-family:Helvetica,Arial,sans-serif;font-weight:bold}
label.inline{display:inline;font-weight:normal;font-family:Georgia,serif}
form.inline{display:inline}
table{border-collapse:collapse}td{padding:4px 12px 4px 0;vertical-align:top}
"#;

pub fn layout(title: &str, extra_head: &str, active: usize, body: &str) -> String {
    let badge = if active > 0 { format!(" ({active})") } else { String::new() };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style>{CSS}</style>{extra_head}</head><body>\
         <div class=\"top\"><a class=\"brand\" href=\"/\">Anna's Kobo</a>\
         <a href=\"/\">Search</a><a href=\"/jobs\">Downloads{badge}</a><a href=\"/settings\">Settings</a></div>\
         {body}</body></html>",
        esc(title)
    )
}

fn ext_options(selected: &str) -> String {
    let mut s = String::new();
    for (v, l) in [("", "Any format"), ("epub", "EPUB"), ("pdf", "PDF"), ("mobi", "MOBI"), ("azw3", "AZW3"), ("cbz", "CBZ")] {
        let sel = if v == selected { " selected" } else { "" };
        s.push_str(&format!("<option value=\"{v}\"{sel}>{l}</option>"));
    }
    s
}

pub fn search_form(q: &str, ext: &str) -> String {
    format!(
        "<form method=\"get\" action=\"/search\">\
         <input type=\"text\" name=\"q\" value=\"{}\" placeholder=\"Title, author, ISBN\" autofocus>\
         <select name=\"ext\">{}</select>\
         <button type=\"submit\">Search</button></form>",
        esc(q),
        ext_options(ext)
    )
}

pub fn home(cfg: &Config, active: usize, annas: Option<String>, libgen: Option<String>, msg: &str) -> String {
    let mut body = String::new();
    if !msg.is_empty() {
        body.push_str(&format!("<div class=\"box\">{}</div>", esc(msg)));
    }
    body.push_str("<h1>Find a book</h1>");
    body.push_str(&search_form("", "epub"));
    let key = if cfg.secret_key.trim().is_empty() {
        "none, downloads use LibGen's free servers".to_string()
    } else {
        "set, downloads use Anna's fast servers".to_string()
    };
    let annas = if !cfg.base_url.trim().is_empty() {
        format!("{} (fixed in Settings)", esc(&cfg.base_url))
    } else {
        annas.map(|h| format!("{h} (auto)")).unwrap_or_else(|| "picked from open-slum.org on first use".into())
    };
    let libgen = libgen.map(|h| format!("{h} (auto)")).unwrap_or_else(|| "picked on first use".into());
    body.push_str(&format!(
        "<div class=\"box muted\"><table>\
         <tr><td>Anna's key</td><td>{key}</td></tr>\
         <tr><td>Anna's mirror</td><td>{annas}</td></tr>\
         <tr><td>LibGen mirror</td><td>{libgen}</td></tr>\
         <tr><td>After download</td><td>{}</td></tr>\
         <tr><td>Kepub</td><td>{}</td></tr></table></div>",
        if cfg.deliver == "folder" { format!("saved to {}", esc(&cfg.download_dir)) } else { "added to the library through the browser".to_string() },
        if cfg.kepubify { "convert EPUBs on download" } else { "off" }
    ));
    body.push_str(
        "<form method=\"post\" action=\"/quit\" class=\"inline\"><button class=\"small\" type=\"submit\">Quit background service</button></form>",
    );
    layout("Anna's Kobo", "", active, &body)
}

pub fn results(q: &str, ext: &str, source: &str, note: &str, books: &[Book], active: usize) -> String {
    let mut body = String::new();
    body.push_str(&search_form(q, ext));
    body.push_str(&format!(
        "<h2>{} result{} for \u{201c}{}\u{201d} via {}</h2>",
        books.len(),
        if books.len() == 1 { "" } else { "s" },
        esc(q),
        esc(source)
    ));
    if !note.is_empty() {
        body.push_str(&format!("<div class=\"box muted\">{}</div>", esc(note)));
    }
    if books.is_empty() {
        body.push_str("<p>Nothing found. Try fewer words, or another format.</p>");
    }
    for b in books {
        let mut line = Vec::new();
        if !b.authors.is_empty() {
            line.push(esc(&b.authors).to_string());
        }
        if !b.publisher.is_empty() {
            line.push(esc(&b.publisher).to_string());
        }
        if !b.year.is_empty() {
            line.push(esc(&b.year).to_string());
        }
        let mut tags = String::new();
        if !b.format.is_empty() {
            tags.push_str(&format!("<span class=\"tag b\">{}</span>", esc(&b.format)));
        }
        if !b.size.is_empty() {
            tags.push_str(&format!("<span class=\"tag\">{}</span>", esc(&b.size)));
        }
        if !b.language.is_empty() {
            tags.push_str(&format!("<span class=\"tag\">{}</span>", esc(&b.language)));
        }
        body.push_str(&format!(
            "<div class=\"book\"><div class=\"title\">{}</div><div class=\"meta\">{}</div><div>{tags}</div>\
             <form method=\"post\" action=\"/download\" style=\"margin-top:8px\">\
             <input type=\"hidden\" name=\"md5\" value=\"{}\"><button type=\"submit\">Download</button></form></div>",
            esc(&b.title),
            line.join(" \u{00b7} "),
            esc(&b.md5)
        ));
    }
    layout(&format!("{q} - Anna's Kobo"), "", active, &body)
}

pub fn searching(q: &str, ext: &str, secs: u64, mirrors_known: bool, active: usize) -> String {
    let target = format!(
        "/search?q={}&ext={}",
        form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>(),
        form_urlencoded::byte_serialize(ext.as_bytes()).collect::<String>()
    );
    let refresh = format!("<meta http-equiv=\"refresh\" content=\"3;url={}\">", html_escape::encode_double_quoted_attribute(&target));
    let hint = if secs == 0 {
        "Contacting the library\u{2026}".to_string()
    } else if !mirrors_known && secs < 20 {
        format!("Picking a mirror from open-slum.org\u{2026} {secs} s")
    } else if secs < 45 {
        format!("Waiting for the library\u{2026} {secs} s")
    } else {
        format!("Still waiting ({secs} s). Mirrors are slow right now; this gives up after a minute or two.")
    };
    let body = format!(
        "<h1>Searching for \u{201c}{}\u{201d}</h1><div class=\"box\">{}</div>\
         <p class=\"muted\">This page refreshes by itself. <a href=\"{}\">Refresh now</a> \u{00b7} <a href=\"/\">Cancel</a></p>",
        esc(q),
        esc(&hint),
        html_escape::encode_double_quoted_attribute(&target)
    );
    layout(&format!("Searching - Anna's Kobo"), &refresh, active, &body)
}

pub fn error(title: &str, msg: &str, active: usize) -> String {
    let body = format!("<h1>{}</h1><div class=\"box\">{}</div><p><a class=\"btn\" href=\"/\">Back</a></p>", esc(title), esc(msg));
    layout(title, "", active, &body)
}

fn human(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{} KB", n >> 10)
    } else {
        format!("{n} B")
    }
}

pub fn jobs(
    jobs: &[Job],
    active: usize,
    import_msg: &str,
    nickel: bool,
    on_kobo: bool,
    browser_mode: bool,
    deliver: Option<u64>,
) -> String {
    let refresh = if let Some(id) = deliver {
        // Hand the finished book to the browser; Nickel adds it to the library.
        format!("<meta http-equiv=\"refresh\" content=\"1;url=/file/{id}\">")
    } else if active > 0 {
        "<meta http-equiv=\"refresh\" content=\"3\">".to_string()
    } else {
        String::new()
    };
    let mut body = String::from("<h1>Downloads</h1>");
    if let Some(id) = deliver {
        let title = jobs.iter().find(|j| j.id == id).map(|j| j.book.title.clone()).unwrap_or_default();
        body.push_str(&format!(
            "<div class=\"box\">Sending \u{201c}{}\u{201d} to your library\u{2026} Tap Continue in the download dialog. Nickel then adds the book in the background; it shows up in My Books within a minute or two. If no dialog appears, tap <a href=\"/file/{id}\">Add to library</a>.</div>",
            esc(&title)
        ));
    }
    if !import_msg.is_empty() {
        body.push_str(&format!("<div class=\"box\">{}</div>", esc(import_msg)));
    }
    if jobs.is_empty() {
        body.push_str("<p>Nothing yet. Search for a book and tap Download.</p>");
    }
    for j in jobs {
        let mut status = j.state.label().to_string();
        let mut bar = String::new();
        match j.state {
            State::Downloading => {
                if let Some(t) = j.total.filter(|t| *t > 0) {
                    let pct = (j.bytes * 100 / t).min(100);
                    status = format!("downloading {} of {} ({pct}%)", human(j.bytes), human(t));
                    bar = format!("<div class=\"bar\"><div style=\"width:{pct}%\"></div></div>");
                } else {
                    status = format!("downloading {}", human(j.bytes));
                }
            }
            State::Done => {
                if let Some(p) = &j.path {
                    status = format!(
                        "done: {}",
                        p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
                    );
                }
            }
            _ => {}
        }
        let detail = if j.detail.is_empty() {
            String::new()
        } else {
            format!("<div class=\"muted\">{}</div>", esc(&j.detail))
        };
        let action = if browser_mode && j.state == State::Done {
            format!(
                "<p><a class=\"btn\" href=\"/file/{}\">{}</a></p>",
                j.id,
                if j.delivered { "Add to library again" } else { "Add to library" }
            )
        } else if j.state == State::Failed {
            format!(
                "<form method=\"post\" action=\"/jobs/retry\" style=\"margin-top:8px\"><input type=\"hidden\" name=\"id\" value=\"{}\"><button type=\"submit\">Retry</button></form>",
                j.id
            )
        } else {
            String::new()
        };
        body.push_str(&format!(
            "<div class=\"book\"><div class=\"title\">{}</div><div class=\"meta\">{}</div>{bar}{detail}{action}</div>",
            esc(&j.book.title),
            esc(&status)
        ));
    }
    body.push_str("<p style=\"margin-top:16px\">");
    if nickel && !browser_mode {
        body.push_str(
            "<form method=\"post\" action=\"/import\" class=\"inline\"><button type=\"submit\">Import into library now</button></form> ",
        );
    }
    body.push_str(
        "<form method=\"post\" action=\"/jobs/clear\" class=\"inline\"><button class=\"small\" type=\"submit\">Clear finished</button></form></p>",
    );
    if browser_mode {
        if on_kobo {
            body.push_str("<p class=\"muted\">Finished books are handed to the Kobo browser (tap Continue). Nickel adds each one in the background, about a minute per book; then it is in My Books. \u{201c}Add to library again\u{201d} makes a second copy.</p>");
        }
    } else if nickel {
        body.push_str("<p class=\"muted\">Finished books are imported automatically when the queue is empty. Close this window afterwards to see them in My Books.</p>");
    } else if on_kobo {
        body.push_str(&format!("<p class=\"muted\">To see finished books in your library, {}.</p>", crate::nickel::MANUAL_HINT));
    }
    layout("Downloads - Anna's Kobo", &refresh, active, &body)
}

fn checked(b: bool) -> &'static str {
    if b { " checked" } else { "" }
}

pub fn settings(cfg: &Config, saved: bool, active: usize, data_dir: &str) -> String {
    let mut body = String::from("<h1>Settings</h1>");
    if saved {
        body.push_str("<div class=\"box\">Saved.</div>");
    }
    let src = |v: &str| if cfg.search_source == v { " selected" } else { "" };
    body.push_str(&format!(
        "<form method=\"post\" action=\"/settings\">\
         <label>Anna's Archive secret key</label>\
         <input type=\"text\" name=\"secret_key\" value=\"{}\" placeholder=\"from annas-archive.gl/account\">\
         <div class=\"muted\">Optional. A membership key enables Anna's fast servers; without one, books come from LibGen's free servers (same files).</div>\
         <label>Anna's Archive mirror (blank = auto from open-slum.org)</label>\
         <input type=\"text\" name=\"base_url\" value=\"{}\" placeholder=\"annas-archive.gl\">\
         <label>Search source</label>\
         <select name=\"search_source\">\
           <option value=\"auto\"{}>Anna's Archive, LibGen if Anna's needs a browser check</option>\
           <option value=\"annas\"{}>Anna's Archive only</option>\
           <option value=\"libgen\"{}>LibGen only</option></select>\
         <label>LibGen mirror (blank = auto)</label>\
         <input type=\"text\" name=\"libgen_url\" value=\"{}\" placeholder=\"libgen.li\">\
         <label>After download</label>\
         <select name=\"deliver\">\
           <option value=\"browser\"{}>Add to the library through the Kobo browser (instant)</option>\
           <option value=\"folder\"{}>Save to the folder below, import with NickelMenu</option></select>\
         <label>Download folder (folder mode)</label>\
         <input type=\"text\" name=\"download_dir\" value=\"{}\">\
         <label>Results per search</label>\
         <input type=\"text\" name=\"max_results\" value=\"{}\">\
         <p><input type=\"checkbox\" name=\"kepubify\" id=\"k\" value=\"1\"{}> <label class=\"inline\" for=\"k\">Convert EPUB to KEPUB after download</label></p>\
         <p><input type=\"checkbox\" name=\"auto_import\" id=\"a\" value=\"1\"{}> <label class=\"inline\" for=\"a\">Import into the library automatically</label></p>\
         <p><button type=\"submit\">Save</button></p></form>\
         <p class=\"muted\">Config file: {}/config.json \u{00b7} version {}</p>",
        esc(&cfg.secret_key),
        esc(&cfg.base_url),
        src("auto"),
        src("annas"),
        src("libgen"),
        esc(&cfg.libgen_url),
        if cfg.deliver != "folder" { " selected" } else { "" },
        if cfg.deliver == "folder" { " selected" } else { "" },
        esc(&cfg.download_dir),
        cfg.max_results,
        checked(cfg.kepubify),
        checked(cfg.auto_import),
        esc(data_dir),
        crate::VERSION
    ));
    layout("Settings - Anna's Kobo", "", active, &body)
}
