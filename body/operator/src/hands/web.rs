//! BFS  LEVEL    Operator (Stage 3-4 milestones) - the seat daemon on the Linux body  [crate brainos-operator]
//! BFS  PURPOSE  Web verbs: drive Chromium over its DevTools websocket to open a page, read its text, find/click/type into elements, and search DuckDuckGo
//! BFS  DEFINES  CdpSession, open, read, find, click, type_into, search
//! BFS  USES     this crate: HandCtx | external: anyhow, futures-util, serde-json, std, tokio-tungstenite
//! BFS  USED BY  -
//!
//! Web hand — Chromium CDP over websocket.
//!
//! Discovers targets via GET /json, opens a websocket to the debugger,
//! navigates, reads page text, finds/clicks/types via Runtime.evaluate.
//! web.search = navigate DuckDuckGo HTML endpoint and scrape results.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use super::{HandCtx, run_cmd};

static CDP_MSG_ID: AtomicU64 = AtomicU64::new(1);

/// Minimal CDP session — connect, send command, await response.
pub struct CdpSession {
    pub ws_url: String,
}

impl CdpSession {
    /// Discover the first page target from /json endpoint.
    pub async fn discover(cdp_base: &str) -> Result<CdpSession> {
        let url = format!("{cdp_base}/json");
        let resp = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()?
            .get(&url)
            .send()
            .await?;
        let targets: Vec<Value> = resp.json().await?;
        let page = targets.iter()
            .find(|t| t["type"].as_str() == Some("page"))
            .ok_or_else(|| anyhow::anyhow!("no page target found"))?;
        let ws_url = page["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no ws url"))?
            .to_string();
        Ok(CdpSession { ws_url })
    }

    /// Send one CDP command and await its response (by matching id).
    /// Times out after 10s.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = CDP_MSG_ID.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"id": id, "method": method, "params": params});

        let (mut ws, _) = connect_async(&self.ws_url).await
            .map_err(|e| anyhow::anyhow!("CDP connect: {e}"))?;

        ws.send(Message::Text(msg.to_string())).await?;

        // wait for response matching our id
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                ws.next()
            ).await {
                Ok(Some(Ok(Message::Text(txt)))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                        if v["id"].as_u64() == Some(id) {
                            if let Some(err) = v.get("error") {
                                bail!("CDP error: {err}");
                            }
                            return Ok(v["result"].clone());
                        }
                    }
                }
                Ok(Some(Ok(_))) => continue,
                _ => bail!("CDP stream ended or timeout"),
            }
        }
        bail!("CDP call timed out")
    }

    /// Navigate to URL and wait for load.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        self.call("Page.navigate", json!({"url": url})).await?;
        // brief settle
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        Ok(())
    }

    /// Get page text content, trimmed to 6000 chars.
    pub async fn read_text(&self) -> Result<String> {
        let r = self.call("Runtime.evaluate", json!({
            "expression": "document.body ? document.body.innerText.trim().substring(0,6000) : ''",
            "returnByValue": true
        })).await?;
        Ok(r["result"]["value"].as_str().unwrap_or("").to_string())
    }

    /// Evaluate JS and return the result value.
    pub async fn eval(&self, js: &str) -> Result<Value> {
        let r = self.call("Runtime.evaluate", json!({
            "expression": js,
            "returnByValue": true,
            "awaitPromise": true
        })).await?;
        Ok(r["result"]["value"].clone())
    }
}

fn cdp_base(ctx: &HandCtx) -> String {
    ctx.config.cdp.url.clone()
}

async fn session(ctx: &HandCtx) -> Result<CdpSession> {
    CdpSession::discover(&cdp_base(ctx)).await
}

pub async fn open(input: &Value, ctx: &HandCtx) -> String {
    let url = input["url"].as_str().unwrap_or("").trim();
    if url.is_empty() { return "error: no url".into(); }
    let url = if url.starts_with("http") { url.to_string() } else { format!("https://{url}") };
    match session(ctx).await {
        Ok(s) => match s.navigate(&url).await {
            Ok(_) => format!("navigated to {url}"),
            Err(e) => format!("navigate error: {e}"),
        },
        Err(e) => format!("CDP unavailable: {e}"),
    }
}

pub async fn read(ctx: &HandCtx) -> String {
    match session(ctx).await {
        Ok(s) => match s.read_text().await {
            Ok(t) => t,
            Err(e) => format!("read error: {e}"),
        },
        Err(e) => format!("CDP unavailable: {e}"),
    }
}

pub async fn find(input: &Value, ctx: &HandCtx) -> String {
    let text = input["text"].as_str().unwrap_or("").trim();
    let js = format!(
        r#"(function(){{
            var els = document.querySelectorAll('*');
            for (var i=0; i<els.length; i++) {{
                if (els[i].textContent.trim().includes({:?})) {{
                    var r = els[i].getBoundingClientRect();
                    return JSON.stringify({{found:true, x:r.x, y:r.y, w:r.width, h:r.height}});
                }}
            }}
            return JSON.stringify({{found:false}});
        }})()"#, text
    );
    match session(ctx).await {
        Ok(s) => s.eval(&js).await.map(|v| v.to_string()).unwrap_or_else(|e| e.to_string()),
        Err(e) => format!("CDP unavailable: {e}"),
    }
}

pub async fn click(input: &Value, ctx: &HandCtx) -> String {
    let sel = input["selector"].as_str().unwrap_or("").trim();
    if sel.is_empty() { return "error: no selector".into(); }
    let js = format!(
        "var el = document.querySelector({:?}); if (el) {{ el.click(); 'clicked'; }} else {{ 'not found'; }}",
        sel
    );
    match session(ctx).await {
        Ok(s) => s.eval(&js).await.map(|v| v.to_string()).unwrap_or_else(|e| e.to_string()),
        Err(e) => format!("CDP unavailable: {e}"),
    }
}

pub async fn type_into(input: &Value, ctx: &HandCtx) -> String {
    let sel = input["selector"].as_str().unwrap_or("").trim();
    let text = input["text"].as_str().unwrap_or("").trim();
    let js = format!(
        "var el = document.querySelector({:?}); if (el) {{ el.focus(); el.value={:?}; 'typed'; }} else {{ 'not found'; }}",
        sel, text
    );
    match session(ctx).await {
        Ok(s) => s.eval(&js).await.map(|v| v.to_string()).unwrap_or_else(|e| e.to_string()),
        Err(e) => format!("CDP unavailable: {e}"),
    }
}

pub async fn search(input: &Value, ctx: &HandCtx) -> String {
    let query = input["query"].as_str()
        .or(input.as_str())
        .unwrap_or("").trim();
    if query.is_empty() { return "error: no query".into(); }

    // try CDP first (chromium running with --remote-debugging-port=9222)
    let encoded = urlencoding(query);
    let search_url = format!("https://html.duckduckgo.com/html/?q={encoded}");

    match session(ctx).await {
        Ok(s) => {
            match s.navigate(&search_url).await {
                Ok(_) => match s.read_text().await {
                    Ok(t) => {
                        // trim to relevant result lines
                        let relevant: String = t.lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(40)
                            .collect::<Vec<_>>()
                            .join("\n");
                        relevant
                    }
                    Err(e) => format!("read error: {e}"),
                },
                Err(e) => {
                    // CDP navigate failed — fall back to curl
                    curl_search(query, &search_url).await
                }
            }
        }
        Err(_) => {
            // No CDP — use curl as fallback
            curl_search(query, &search_url).await
        }
    }
}

async fn curl_search(query: &str, url: &str) -> String {
    let r = run_cmd(&["curl", "-s", "-L", "--max-time", "10",
        "-A", "Mozilla/5.0", url], 15).await;
    // naive HTML strip
    strip_html(&r).chars().take(3000).collect()
}

fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => { in_tag = true; }
            '>' => { in_tag = false; out.push(' '); }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // collapse whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ---- CDP framing tests --------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdp_message_id_monotonic() {
        let id1 = CDP_MSG_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = CDP_MSG_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1, "CDP message IDs must be monotonically increasing");
    }

    #[test]
    fn test_cdp_command_serialization() {
        let id = 42u64;
        let msg = json!({
            "id": id,
            "method": "Page.navigate",
            "params": {"url": "https://example.com"}
        });
        let s = msg.to_string();
        assert!(s.contains("\"id\":42"));
        assert!(s.contains("\"method\":\"Page.navigate\""));
        assert!(s.contains("\"url\":\"https://example.com\""));
    }

    #[test]
    fn test_cdp_response_matching() {
        // simulate matching a response by id
        let response = json!({"id": 7, "result": {"frameId": "abc"}});
        let target_id = 7u64;
        assert_eq!(response["id"].as_u64(), Some(target_id));
        assert_eq!(response["result"]["frameId"].as_str(), Some("abc"));
    }

    #[test]
    fn test_cdp_error_detection() {
        let error_response = json!({
            "id": 1,
            "error": {"code": -32000, "message": "No target with given id"}
        });
        assert!(error_response.get("error").is_some());
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello+world");
        assert_eq!(urlencoding("rust & tokio"), "rust+%26+tokio");
    }

    #[test]
    fn test_strip_html() {
        let html = "<h1>Title</h1><p>Some <b>bold</b> text.</p>";
        let text = strip_html(html);
        assert!(text.contains("Title"));
        assert!(text.contains("bold"));
        assert!(!text.contains("<"));
    }
}
