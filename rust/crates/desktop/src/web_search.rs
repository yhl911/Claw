use serde::Deserialize;

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    title: String,
    description: Option<String>,
    url: String,
}

/// Perform a web search. Returns formatted markdown with top results.
/// If no API key is configured, returns an informative error.
pub fn search(query: &str, api_key: &str) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err(
            "Web search requires a Brave Search API key. \
             Get a free key at https://brave.com/search/api/ \
             and add it in Settings → Brave API Key."
                .to_string(),
        );
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let resp = client
            .get("https://api.search.brave.com/res/v1/web/search")
            .query(&[("q", query), ("count", "8"), ("search_lang", "zh-hans")])
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("web_search request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Brave API error {status}: {body}"));
        }

        let data: BraveResponse = resp
            .json()
            .await
            .map_err(|e| format!("web_search parse error: {e}"))?;

        let results = data.web.map(|w| w.results).unwrap_or_default();
        if results.is_empty() {
            return Ok(format!("No results found for: {query}"));
        }

        let mut out = format!("## Web search: {query}\n\n");
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("{}. **{}**\n", i + 1, r.title));
            if let Some(desc) = &r.description {
                out.push_str(&format!("   {desc}\n"));
            }
            out.push_str(&format!("   <{}>\n\n", r.url));
        }
        Ok(out)
    })
}
