//! Voice feature handlers.
//!
//! Supports three Whisper backends (priority order):
//!   1. OpenAI remote API  — set OPENAI_API_KEY
//!   2. Local compatible   — set WHISPER_URL (e.g. faster-whisper-server)
//!   3. Disabled           — leave both empty
//!
//! AudD song recognition  — set AUDD_API_KEY

use reqwest::multipart;
use regex::Regex;

/// Transcribe an OGG audio file using the configured Whisper backend.
/// Returns the transcribed text or an error string.
pub async fn transcribe(
    http: &reqwest::Client,
    audio_bytes: Vec<u8>,
    openai_key: &str,
    whisper_url: &str,
) -> Result<String, String> {
    // Determine endpoint and auth header
    let (endpoint, auth) = if !openai_key.is_empty() {
        (
            "https://api.openai.com/v1/audio/transcriptions".to_string(),
            format!("Bearer {}", openai_key),
        )
    } else if !whisper_url.is_empty() {
        (whisper_url.to_string(), String::new())
    } else {
        return Err("Voice search is not configured.".to_string());
    };

    let part = multipart::Part::bytes(audio_bytes)
        .file_name("audio.ogg")
        .mime_str("audio/ogg")
        .map_err(|e| e.to_string())?;

    let form = multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1")
        .text("response_format", "text");

    let mut req = http.post(&endpoint).multipart(form);
    if !auth.is_empty() {
        req = req.header("Authorization", auth);
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Whisper API error: {}", resp.status()));
    }

    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(text.trim().to_string())
}

/// Recognition result from AudD.
/// Prefer deezer_url → song_link (Odesli) → spotify_url → text search, in that order.
pub struct RecognitionResult {
    pub title: String,
    pub artist: String,
    pub deezer_url: Option<String>,
    pub spotify_url: Option<String>,
    pub song_link: Option<String>,
}

/// Identify a song from audio bytes using the AudD API.
/// Returns RecognitionResult or an error string.
pub async fn recognize(
    http: &reqwest::Client,
    audio_bytes: Vec<u8>,
    audd_key: &str,
) -> Result<RecognitionResult, String> {
    if audd_key.is_empty() {
        return Err("Song recognition is not configured.".to_string());
    }

    let part = multipart::Part::bytes(audio_bytes)
        .file_name("audio.ogg")
        .mime_str("audio/ogg")
        .map_err(|e| e.to_string())?;

    let form = multipart::Form::new()
        .part("file", part)
        .text("api_token", audd_key.to_string())
        .text("return", "deezer,spotify");

    let resp = http
        .post("https://api.audd.io/")
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if data["status"] != "success" {
        return Err("AudD could not identify the song.".to_string());
    }

    let result = &data["result"];
    if result.is_null() {
        return Err("Song not recognized. Try a longer clip.".to_string());
    }

    let title = result["title"].as_str().unwrap_or("").to_string();
    let artist = result["artist"].as_str().unwrap_or("").to_string();

    if title.is_empty() {
        return Err("Song not recognized.".to_string());
    }

    let deezer_url = result["deezer"]["link"]
        .as_str()
        .map(|s| s.to_string());

    let spotify_url = result["spotify"]["external_urls"]["spotify"]
        .as_str()
        .map(|s| s.to_string());

    let song_link = result["song_link"].as_str().map(|s| s.to_string());

    log::info!(
        "AudD recognized: title={:?} artist={:?} deezer_url={:?} spotify_url={:?} song_link={:?}",
        title, artist, deezer_url, spotify_url, song_link
    );
    log::info!("AudD raw result keys: {:?}", result.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    log::info!("AudD deezer field: {}", result["deezer"]);
    log::info!("AudD spotify field: {}", result["spotify"]);

    Ok(RecognitionResult { title, artist, deezer_url, spotify_url, song_link })
}

/// Scrape a song.link/lis.tn page for a direct Deezer URL via __NEXT_DATA__ JSON.
/// The Odesli API rejects lis.tn vanity URLs, so we parse the page HTML instead.
pub async fn lookup_deezer_via_odesli(http: &reqwest::Client, song_link: &str) -> Option<String> {
    log::info!("Scraping song.link page: {}", song_link);

    let resp = http.get(song_link).send().await.ok()?;
    let status = resp.status();
    if !status.is_success() {
        log::info!("song.link page fetch failed: {}", status);
        return None;
    }
    let html = resp.text().await.ok()?;

    let re = Regex::new(r#"<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).ok()?;
    let caps = re.captures(&html);
    log::info!("song.link __NEXT_DATA__ regex matched: {}", caps.is_some());
    let json_str = caps?.get(1)?.as_str().to_string();

    let data: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    let page_props = &data["props"]["pageProps"];

    // Try the two known shapes of song.link __NEXT_DATA__
    let deezer_url =
        page_props["data"]["linksByPlatform"]["deezer"]["url"].as_str()
        .or_else(|| page_props["pageData"]["linksByPlatform"]["deezer"]["url"].as_str())
        .map(|s| s.to_string());

    if deezer_url.is_none() {
        let keys: Vec<&str> = page_props.as_object()
            .map(|o| o.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        log::info!("song.link __NEXT_DATA__ pageProps keys: {:?}", keys);
    }

    log::info!("song.link scraped Deezer URL: {:?}", deezer_url);
    deezer_url
}

/// Search iTunes for title+artist, then pass the Apple Music URL to Odesli to get a Deezer link.
/// iTunes is free/no-auth and globally indexed; Odesli accepts Apple Music URLs.
pub async fn lookup_deezer_via_itunes(http: &reqwest::Client, title: &str, artist: &str) -> Option<String> {
    let term = format!("{} {}", title, artist);
    log::info!("iTunes search: {:?}", term);

    let resp = match http
        .get("https://itunes.apple.com/search")
        .query(&[("term", term.as_str()), ("media", "music"), ("entity", "song"), ("limit", "1")])
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => { log::info!("iTunes request failed: {}", e); return None; }
    };

    log::info!("iTunes response status: {}", resp.status());
    let data: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => { log::info!("iTunes JSON parse failed: {}", e); return None; }
    };
    log::info!("iTunes resultCount: {}", data["resultCount"]);

    let track_url = match data["results"][0]["trackViewUrl"].as_str() {
        Some(u) => u.to_string(),
        None => { log::info!("iTunes: no trackViewUrl in first result"); return None; }
    };
    log::info!("iTunes track URL: {}", track_url);

    let resp = http
        .get("https://api.song.link/v1-alpha.1/links")
        .query(&[("url", track_url.as_str())])
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        log::info!("Odesli via iTunes failed: {}", resp.status());
        return None;
    }

    let data: serde_json::Value = resp.json().await.ok()?;
    let deezer_url = data["linksByPlatform"]["deezer"]["url"].as_str().map(|s| s.to_string());
    log::info!("iTunes→Odesli Deezer URL: {:?}", deezer_url);
    deezer_url
}
