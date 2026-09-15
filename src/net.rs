use std::sync::OnceLock;
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, ResponseExt};

/// A desktop browser UA; some mirrors refuse obvious bots.
pub const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn build(global: Option<Duration>) -> Agent {
    let cfg = Agent::config_builder()
        .user_agent(UA)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(global)
        .max_redirects(10)
        .tls_config(TlsConfig::builder().root_certs(RootCerts::WebPki).build())
        .build();
    Agent::new_with_config(cfg)
}

/// Agent for pages and API calls: bounded total time.
pub fn agent() -> &'static Agent {
    static A: OnceLock<Agent> = OnceLock::new();
    A.get_or_init(|| build(Some(Duration::from_secs(45))))
}

/// Agent for file downloads: no global timeout (books can be big and the
/// Kobo's Wi-Fi is slow), only the connect timeout.
pub fn download_agent() -> &'static Agent {
    static A: OnceLock<Agent> = OnceLock::new();
    A.get_or_init(|| build(None))
}

#[allow(dead_code)]
pub struct Page {
    pub status: u16,
    pub url: String,
    pub body: String,
}

pub fn get(agent: &Agent, url: &str) -> Result<Page, String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let final_url = resp.get_uri().to_string();
    let body = resp
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok(Page { status, url: final_url, body })
}
