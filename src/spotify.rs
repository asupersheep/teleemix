use regex::Regex;
use reqwest::Client;
use serde_json::Value;

pub struct SpotifyMeta {
    pub query: String,
    pub label: String,
}

/// Resolve a Spotify URL to track title + artist.
/// Uses the Spotify embed page __NEXT_DATA__ JSON — no API key required.
/// Falls back to oEmbed for title if embed page fails.
pub async fn resolve(url: &str) -> Option<SpotifyMeta> {
    let client = Client::new();

    let (title, artist) = get_metadata_from_embed(&client, url).await;

    // If embed page didn't give us a title, try oEmbed
    let title = if title.is_empty() {
        get_title_from_oembed(&client, url).await.unwrap_or_default()
    } else {
        title
    };

    if title.is_empty() {
        return None;
    }

    let query = format!("{} {}", title, artist).trim().to_string();
    let label = if artist.is_empty() {
        title.clone()
    } else {
        format!("{} — {}", title, artist)
    };

    Some(SpotifyMeta { query, label })
}

async fn get_metadata_from_embed(client: &Client, url: &str) -> (String, String) {
    // Convert track/album URL to embed URL
    let embed_url = url
        .replace("open.spotify.com/", "open.spotify.com/embed/")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string()
        + "?utm_source=oembed";

    let resp = match client.get(&embed_url).send().await {
        Ok(r) => r,
        Err(_) => return (String::new(), String::new()),
    };

    let body = match resp.text().await {
        Ok(b) => b,
        Err(_) => return (String::new(), String::new()),
    };

    // Extract __NEXT_DATA__ JSON
    let re = Regex::new(r#"<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#)
        .unwrap();
    let json_str = match re.captures(&body) {
        Some(caps) => caps.get(1).map(|m| m.as_str()).unwrap_or(""),
        None => return (String::new(), String::new()),
    };

    let data: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return (String::new(), String::new()),
    };

    let entity = &data["props"]["pageProps"]["state"]["data"]["entity"];

    let title = entity["name"].as_str().unwrap_or("").to_string();
    let artists: Vec<&str> = entity["artists"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    let artist = artists.join(", ");

    (title, artist)
}

async fn get_title_from_oembed(client: &Client, url: &str) -> Option<String> {
    let resp = client
        .get("https://open.spotify.com/oembed")
        .query(&[("url", url)])
        .send()
        .await
        .ok()?;

    let data: Value = resp.json().await.ok()?;
    data["title"].as_str().map(|s| s.to_string())
}
