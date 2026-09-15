//! Anna's Archive: HTML search (when not behind the DDoS-Guard browser
//! check) and the members-only fast download JSON API.

use crate::net;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Book {
    pub title: String,
    pub authors: String,
    pub publisher: String,
    pub year: String,
    pub language: String,
    pub format: String,
    pub size: String,
    pub md5: String,
    pub source: String,
}

#[derive(Debug)]
pub enum SearchError {
    /// The mirror answered with the DDoS-Guard JavaScript challenge.
    Blocked(String),
    Failed(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Blocked(h) => write!(f, "{h} requires a browser check for search pages"),
            SearchError::Failed(e) => write!(f, "{e}"),
        }
    }
}

fn re(s: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(s).unwrap())
}

static RE_FORMAT: OnceLock<Regex> = OnceLock::new();
static RE_SIZE: OnceLock<Regex> = OnceLock::new();
static RE_LANG: OnceLock<Regex> = OnceLock::new();
static RE_YEAR: OnceLock<Regex> = OnceLock::new();

pub fn is_md5(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn search_url(host: &str, q: &str, ext: &str) -> String {
    let mut url = format!(
        "https://{host}/search?q={}&content=book_any",
        form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
    );
    if !ext.is_empty() {
        url.push_str("&ext=");
        url.push_str(&ext.to_lowercase());
    }
    url
}

pub fn is_challenge(page: &net::Page) -> bool {
    (page.status == 403 || page.status == 503)
        && (page.body.contains("DDoS-Guard") || page.body.contains("js-challenge"))
        || (page.body.contains("ddos-guard/js-challenge") && !page.body.contains("/md5/"))
}

pub fn search(host: &str, q: &str, ext: &str, max: usize) -> Result<Vec<Book>, SearchError> {
    let url = search_url(host, q, ext);
    log::info!("annas search {url}");
    let page = net::get(net::agent(), &url).map_err(SearchError::Failed)?;
    if is_challenge(&page) {
        return Err(SearchError::Blocked(host.to_string()));
    }
    if page.status != 200 {
        return Err(SearchError::Failed(format!("{host} answered HTTP {}", page.status)));
    }
    let mut books = parse_search(&page.body);
    books.truncate(max);
    Ok(books)
}

/// Pulls results out of the search page. Written to survive class-name
/// churn: it keys on `/md5/` links and reads the metadata line ("English
/// [en] · EPUB · 0.7MB · 2015") from the nearest enclosing block.
pub fn parse_search(html: &str) -> Vec<Book> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(r#"a[href^="/md5/"]"#).unwrap();
    let sel_author = Selector::parse(r#"a[href^="/search"] span[class*="user-edit"]"#).unwrap();
    let sel_pub = Selector::parse(r#"a[href^="/search"] span[class*="company"]"#).unwrap();
    let re_format = re(
        r"(?i)\b(EPUB|PDF|MOBI|AZW3|AZW|DJVU|CBZ|CBR|FB2|DOCX?|TXT|RTF|LIT)\b",
        &RE_FORMAT,
    );
    let re_size = re(r"(?i)(\d+(?:\.\d+)?)\s?(MB|KB|GB)", &RE_SIZE);
    let re_lang = re(r"([A-Z][A-Za-z]+(?: [A-Za-z]+)?) \[[a-z]{2,3}\]", &RE_LANG);
    let re_year = re(r"\b(1[5-9]\d\d|20\d\d)\b", &RE_YEAR);

    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for a in doc.select(&sel) {
        let href = a.attr("href").unwrap_or("");
        let md5: String = href
            .trim_start_matches("/md5/")
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if !is_md5(&md5) || seen.contains(&md5) {
            continue;
        }
        let title = clean(&a.text().collect::<String>());
        if title.is_empty() {
            continue;
        }
        // Climb to the block that carries the metadata line.
        let mut block: Option<ElementRef> = None;
        let mut cur = a.parent();
        for _ in 0..6 {
            let Some(n) = cur else { break };
            if let Some(el) = ElementRef::wrap(n) {
                let text = el.text().collect::<String>();
                if text.contains(" · ") && re_format.is_match(&text) {
                    block = Some(el);
                    break;
                }
            }
            cur = n.parent();
        }
        let Some(block) = block else { continue };
        let text = block.text().collect::<Vec<_>>().join(" ");
        let meta_line = text
            .split('\n')
            .find(|l| l.contains(" · ") && re_format.is_match(l))
            .unwrap_or(&text)
            .to_string();
        let format = re_format
            .captures(&meta_line)
            .map(|c| c[1].to_uppercase())
            .unwrap_or_default();
        let size = re_size
            .captures(&meta_line)
            .map(|c| format!("{} {}", &c[1], c[2].to_uppercase()))
            .unwrap_or_default();
        let language = re_lang
            .captures(&meta_line)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let year = re_year
            .captures(&meta_line)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let authors = block
            .select(&sel_author)
            .next()
            .and_then(|s| s.parent().and_then(ElementRef::wrap))
            .map(|p| clean(&p.text().collect::<String>()))
            .unwrap_or_default();
        let publisher = block
            .select(&sel_pub)
            .next()
            .and_then(|s| s.parent().and_then(ElementRef::wrap))
            .map(|p| clean(&p.text().collect::<String>()))
            .unwrap_or_default();
        seen.push(md5.clone());
        out.push(Book {
            title,
            authors,
            publisher,
            year,
            language,
            format,
            size,
            md5,
            source: "Anna's Archive".into(),
        });
    }
    out
}

pub fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize, Debug)]
struct FastDownload {
    download_url: Option<String>,
    error: Option<String>,
    #[serde(default)]
    account_fast_download_info: Option<serde_json::Value>,
}

/// Resolves a direct file URL through the membership API.
pub fn fast_download_url(host: &str, md5: &str, key: &str) -> Result<String, String> {
    if !is_md5(md5) {
        return Err(format!("not an md5: {md5}"));
    }
    if key.trim().is_empty() {
        return Err("no Anna's Archive membership key set".into());
    }
    let url = format!(
        "https://{host}/dyn/api/fast_download.json?md5={md5}&key={}",
        form_urlencoded::byte_serialize(key.trim().as_bytes()).collect::<String>()
    );
    let page = net::get(net::agent(), &url)?;
    let fd: FastDownload = serde_json::from_str(&page.body)
        .map_err(|e| format!("{host} sent something that is not the API JSON (HTTP {}): {e}", page.status))?;
    if let Some(info) = &fd.account_fast_download_info {
        log::info!("fast download quota: {info}");
    }
    match fd.download_url {
        Some(u) if !u.is_empty() => Ok(u),
        _ => Err(format!(
            "Anna's Archive: {}",
            fd.error.unwrap_or_else(|| format!("no download URL (HTTP {})", page.status))
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_result_block() {
        let html = r#"<div class="flex"><a href="/md5/4a702054b356e7ed5364318bc954c99c" class="custom-a block"><img></a>
          <div class="max-w-full"><a href="/md5/4a702054b356e7ed5364318bc954c99c">Dreamer of Dune</a>
          <div class="text-gray-800">✅ English [en] · EPUB · 0.7MB · 2003 · Tor</div>
          <a href="/search?q=Brian"><span class="icon-[mdi--user-edit]"></span>Brian Herbert</a>
          <a href="/search?q=Tor"><span class="icon-[mdi--company]"></span>Tor Books</a></div></div>"#;
        let b = parse_search(html);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].title, "Dreamer of Dune");
        assert_eq!(b[0].format, "EPUB");
        assert_eq!(b[0].size, "0.7 MB");
        assert_eq!(b[0].language, "English");
        assert_eq!(b[0].year, "2003");
        assert_eq!(b[0].authors, "Brian Herbert");
        assert_eq!(b[0].publisher, "Tor Books");
    }
}
