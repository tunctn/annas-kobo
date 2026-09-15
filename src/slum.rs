//! Mirror discovery from the Shadow Library Uptime Monitor (open-slum.org)
//! and a small cache of the mirror actually in use.

use crate::config::{normalize_host, Config};
use crate::net;
use scraper::{Html, Selector};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use ureq::Agent;

pub const SLUM_URL: &str = "https://open-slum.org/";
const DEFAULT_ANNAS: &[&str] = &["annas-archive.gl", "annas-archive.pk", "annas-archive.gd"];
const DEFAULT_LIBGEN: &[&str] = &["libgen.li", "libgen.vg", "libgen.la", "libgen.bz", "libgen.gl"];
const TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    Up,
    Protected,
    Degraded,
    Unknown,
    Down,
}

#[derive(Clone, Debug)]
pub struct Mirror {
    pub host: String,
    pub status: Status,
}

#[derive(Clone, Debug, Default)]
pub struct Slum {
    pub annas: Vec<Mirror>,
    pub libgen: Vec<Mirror>,
}

pub fn fetch(agent: &Agent) -> Result<Slum, String> {
    let page = net::get(agent, SLUM_URL)?;
    if page.status != 200 {
        return Err(format!("open-slum.org answered {}", page.status));
    }
    let s = parse(&page.body);
    if s.annas.is_empty() && s.libgen.is_empty() {
        return Err("open-slum.org page has no mirror list".into());
    }
    Ok(s)
}

pub fn parse(html: &str) -> Slum {
    let doc = Html::parse_document(html);
    let card = Selector::parse("div.site-card").unwrap();
    let title = Selector::parse("a.site-card-title").unwrap();
    let item = Selector::parse("li.domain-item-dense").unwrap();
    let link = Selector::parse("a.domain-link").unwrap();
    let badge = Selector::parse("a.status-badge, span.status-badge").unwrap();
    let mut out = Slum::default();
    for c in doc.select(&card) {
        let name = c
            .select(&title)
            .next()
            .map(|t| t.text().collect::<String>().trim().to_lowercase())
            .unwrap_or_default();
        let mut mirrors = Vec::new();
        for li in c.select(&item) {
            let Some(href) = li.select(&link).next().and_then(|a| a.attr("href")) else {
                continue;
            };
            let host = normalize_host(href);
            if host.is_empty() {
                continue;
            }
            let mut status = Status::Unknown;
            for b in li.select(&badge) {
                for cls in b.value().classes() {
                    status = match cls {
                        "up" => Status::Up,
                        "protected" => Status::Protected,
                        "degraded" => Status::Degraded,
                        "down" => Status::Down,
                        _ => continue,
                    };
                }
            }
            mirrors.push(Mirror { host, status });
        }
        mirrors.sort_by_key(|m| m.status);
        match name.as_str() {
            "annas" | "anna's archive" | "annas-archive" => out.annas = mirrors,
            "libgen" | "library genesis" => out.libgen = mirrors,
            _ => {}
        }
    }
    out
}

fn probe_annas(host: &str) -> bool {
    let url = format!(
        "https://{host}/dyn/api/fast_download.json?md5=00000000000000000000000000000000&key=probe"
    );
    match net::get(net::agent(), &url) {
        Ok(p) => p.body.contains("download_url"),
        Err(e) => {
            log::info!("probe {host}: {e}");
            false
        }
    }
}

fn probe_libgen(host: &str) -> bool {
    let url = format!("https://{host}/index.php?req=probe&objects%5B%5D=f&res=25");
    match net::get(net::agent(), &url) {
        Ok(p) => p.status == 200 && p.body.to_lowercase().contains("library genesis"),
        Err(e) => {
            log::info!("probe {host}: {e}");
            false
        }
    }
}

struct Cached {
    host: String,
    at: Instant,
}

#[derive(Default)]
struct State {
    slum: Option<(Instant, Slum)>,
    annas: Option<Cached>,
    libgen: Option<Cached>,
    /// When Anna's search last answered with the browser check.
    annas_blocked: Option<Instant>,
}

/// How long to skip Anna's search after seeing the browser check.
const BLOCKED_TTL: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Resolver {
    st: Mutex<State>,
}

impl Resolver {
    pub fn new() -> Self {
        Self { st: Mutex::new(State::default()) }
    }

    fn slum(&self) -> Slum {
        let st = self.st.lock().unwrap();
        if let Some((at, s)) = &st.slum {
            if at.elapsed() < TTL {
                return s.clone();
            }
        }
        drop(st);
        let s = match fetch(net::agent()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("slum: {e}");
                Slum::default()
            }
        };
        let mut st = self.st.lock().unwrap();
        st.slum = Some((Instant::now(), s.clone()));
        s
    }

    fn candidates(list: &[Mirror], defaults: &[&str], skip_prefix: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in list {
            if m.status == Status::Down || (!skip_prefix.is_empty() && m.host.starts_with(skip_prefix)) {
                continue;
            }
            if !out.contains(&m.host) {
                out.push(m.host.clone());
            }
        }
        for d in defaults {
            if !out.contains(&d.to_string()) {
                out.push(d.to_string());
            }
        }
        out
    }

    /// Host for Anna's Archive. Config override wins; otherwise the best
    /// mirror listed on SLUM that answers the JSON API, cached for a while.
    pub fn annas_host(&self, cfg: &Config) -> String {
        if !cfg.base_url.trim().is_empty() {
            return normalize_host(&cfg.base_url);
        }
        if let Some(c) = &self.st.lock().unwrap().annas {
            if c.at.elapsed() < TTL {
                return c.host.clone();
            }
        }
        let slum = self.slum();
        let cands = Self::candidates(&slum.annas, DEFAULT_ANNAS, "software.");
        let host = cands
            .iter()
            .find(|h| probe_annas(h))
            .cloned()
            .unwrap_or_else(|| DEFAULT_ANNAS[0].to_string());
        log::info!("using Anna's Archive mirror {host}");
        self.st.lock().unwrap().annas = Some(Cached { host: host.clone(), at: Instant::now() });
        host
    }

    pub fn libgen_host(&self, cfg: &Config) -> String {
        if !cfg.libgen_url.trim().is_empty() {
            return normalize_host(&cfg.libgen_url);
        }
        if let Some(c) = &self.st.lock().unwrap().libgen {
            if c.at.elapsed() < TTL {
                return c.host.clone();
            }
        }
        let slum = self.slum();
        let cands = Self::candidates(&slum.libgen, DEFAULT_LIBGEN, "");
        let host = cands
            .iter()
            .find(|h| probe_libgen(h))
            .cloned()
            .unwrap_or_else(|| DEFAULT_LIBGEN[0].to_string());
        log::info!("using LibGen mirror {host}");
        self.st.lock().unwrap().libgen = Some(Cached { host: host.clone(), at: Instant::now() });
        host
    }

    /// Ranked LibGen hosts to try in turn: the override or cached choice
    /// first, then everything SLUM lists, then the built-in defaults.
    pub fn libgen_candidates(&self, cfg: &Config) -> Vec<String> {
        let mut out = Vec::new();
        if !cfg.libgen_url.trim().is_empty() {
            out.push(normalize_host(&cfg.libgen_url));
        }
        if let Some(c) = &self.st.lock().unwrap().libgen {
            if !out.contains(&c.host) {
                out.push(c.host.clone());
            }
        }
        let slum = self.slum();
        for h in Self::candidates(&slum.libgen, DEFAULT_LIBGEN, "") {
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out
    }

    /// Fetches the mirror list ahead of the first search.
    pub fn warm(&self) {
        let _ = self.slum();
    }

    /// Remember that a LibGen host just worked.
    pub fn set_libgen(&self, host: &str) {
        self.st.lock().unwrap().libgen = Some(Cached { host: host.to_string(), at: Instant::now() });
    }

    pub fn mark_annas_blocked(&self) {
        self.st.lock().unwrap().annas_blocked = Some(Instant::now());
    }

    /// Minutes since Anna's search was last seen blocked, if that was recent.
    pub fn annas_blocked_for(&self) -> Option<u64> {
        let st = self.st.lock().unwrap();
        st.annas_blocked
            .filter(|t| t.elapsed() < BLOCKED_TTL)
            .map(|t| t.elapsed().as_secs() / 60)
    }

    /// Forget the chosen Anna's mirror so the next call probes again.
    pub fn forget_annas(&self) {
        self.st.lock().unwrap().annas = None;
    }

    pub fn forget_libgen(&self) {
        self.st.lock().unwrap().libgen = None;
    }

    /// What is currently cached, for the status line in the UI.
    pub fn current(&self) -> (Option<String>, Option<String>) {
        let st = self.st.lock().unwrap();
        (
            st.annas.as_ref().map(|c| c.host.clone()),
            st.libgen.as_ref().map(|c| c.host.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flatmonitor_page() {
        let html = r##"
        <div class="site-card"><div class="site-card-header"><a href="annas.html" class="site-card-title">annas</a></div>
        <ul class="domain-list-dense">
          <li class="domain-item-dense"><a href="https://annas-archive.gl" class="domain-link">annas-archive.gl</a>
            <div class="status-col"><a href="#" class="status-badge compact degraded">DEGRADED</a></div></li>
          <li class="domain-item-dense"><a href="https://annas-archive.pk" class="domain-link">annas-archive.pk</a>
            <div class="status-col"><a href="#" class="status-badge compact up">UP</a></div></li>
        </ul></div>
        <div class="site-card"><a href="libgen.html" class="site-card-title">libgen</a>
        <ul><li class="domain-item-dense"><a href="https://libgen.li/" class="domain-link">libgen.li</a>
            <a class="status-badge compact protected">PROTECTED</a></li></ul></div>"##;
        let s = parse(html);
        assert_eq!(s.annas.len(), 2);
        assert_eq!(s.annas[0].host, "annas-archive.pk");
        assert_eq!(s.annas[0].status, Status::Up);
        assert_eq!(s.libgen[0].host, "libgen.li");
        assert_eq!(s.libgen[0].status, Status::Protected);
    }
}
