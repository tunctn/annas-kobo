//! LibGen (libgen.li engine) HTML search. Same md5 identifiers as Anna's
//! Archive, so results download through Anna's fast servers.

use crate::annas::{clean, is_md5, Book};
use crate::net;
use scraper::{Html, Selector};

pub fn search_url(host: &str, q: &str) -> String {
    format!(
        "https://{host}/index.php?req={}&columns%5B%5D=t&columns%5B%5D=a&columns%5B%5D=s&objects%5B%5D=f&topics%5B%5D=l&topics%5B%5D=f&topics%5B%5D=c&res=25&filesuns=all",
        form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
    )
}

pub fn search(host: &str, q: &str, ext: &str, max: usize) -> Result<Vec<Book>, String> {
    let url = search_url(host, q);
    log::info!("libgen search {url}");
    let page = net::get(net::agent(), &url)?;
    if page.status != 200 {
        return Err(format!("{host} answered HTTP {}", page.status));
    }
    let mut books = parse_search(&page.body);
    if !ext.is_empty() {
        let e = ext.to_lowercase();
        books.retain(|b| b.format.to_lowercase() == e);
    }
    books.truncate(max);
    Ok(books)
}

pub fn parse_search(html: &str) -> Vec<Book> {
    let doc = Html::parse_document(html);
    let rows = Selector::parse("table#tablelibgen tbody tr, table.table tbody tr").unwrap();
    let td = Selector::parse("td").unwrap();
    let a = Selector::parse("a").unwrap();
    let mut out = Vec::new();
    for tr in doc.select(&rows) {
        let cells: Vec<_> = tr.select(&td).collect();
        if cells.len() < 9 {
            continue;
        }
        let md5 = cells
            .iter()
            .flat_map(|c| c.select(&a))
            .filter_map(|l| l.attr("href"))
            .filter_map(|h| {
                if let Some(i) = h.find("md5=") {
                    Some(h[i + 4..].chars().take(32).collect::<String>())
                } else if let Some(i) = h.find("/book/") {
                    Some(h[i + 6..].chars().take(32).collect::<String>())
                } else {
                    None
                }
            })
            .map(|s| s.to_lowercase())
            .find(|s| is_md5(s));
        let Some(md5) = md5 else { continue };
        if out.iter().any(|b: &Book| b.md5 == md5) {
            continue;
        }
        // The title anchor's own text; edition notes and ISBNs sit in <i>
        // children we do not want in the file name.
        let title = cells[0]
            .select(&a)
            .map(|l| {
                clean(
                    &l.children()
                        .filter_map(|c| c.value().as_text().map(|t| t.to_string()))
                        .collect::<String>(),
                )
            })
            .find(|t| !t.is_empty() && !t.chars().all(|c| c.is_ascii_digit() || c == ';' || c == ' ' || c == '-'))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let text = |i: usize| clean(&cells[i].text().collect::<String>());
        out.push(Book {
            title,
            authors: text(1).replace("(Author)", "").trim().to_string(),
            publisher: text(2),
            year: text(3),
            language: text(4),
            size: text(6),
            format: text(7).to_uppercase(),
            md5,
            source: "LibGen".into(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_row() {
        let html = r#"<table id="tablelibgen"><tbody><tr>
        <td><a href="edition.php?id=1">Dreamer of Dune <i></i></a><br><a href="edition.php?id=1"><i><font color="green"> 9781429958448</font></i></a></td>
        <td><a href="author.php?id=354895">Herbert, Brian(Author)</a></td><td>Tor</td><td><nobr>2003</nobr></td>
        <td>English</td><td>0</td><td><nobr><a href="/file.php?id=9">704 kB</a></nobr></td><td>epub</td>
        <td><a href="/ads.php?md5=4a702054b356e7ed5364318bc954c99c">1</a></td></tr></tbody></table>"#;
        let b = parse_search(html);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].title, "Dreamer of Dune");
        assert_eq!(b[0].authors, "Herbert, Brian");
        assert_eq!(b[0].format, "EPUB");
        assert_eq!(b[0].size, "704 kB");
        assert_eq!(b[0].md5, "4a702054b356e7ed5364318bc954c99c");
    }
}

/// Free direct download: the ads page carries a `get.php?md5=..&key=..`
/// link that streams the file (after a redirect to a CDN host).
pub fn download_url(host: &str, md5: &str) -> Result<String, String> {
    if !is_md5(md5) {
        return Err(format!("not an md5: {md5}"));
    }
    let url = format!("https://{host}/ads.php?md5={md5}");
    let page = net::get(net::agent(), &url)?;
    if page.status != 200 {
        return Err(format!("{host} answered HTTP {} for the download page", page.status));
    }
    let doc = Html::parse_document(&page.body);
    let a = Selector::parse("a[href*=\"get.php\"]").unwrap();
    let href = doc
        .select(&a)
        .filter_map(|l| l.attr("href"))
        .find(|h| h.contains("md5=") && h.contains("key="))
        .ok_or_else(|| format!("{host} has no download link for {md5} (file not on LibGen?)"))?;
    let href = href.replace("&amp;", "&");
    Ok(if href.starts_with("http") {
        href
    } else {
        format!("https://{host}/{}", href.trim_start_matches('/'))
    })
}
