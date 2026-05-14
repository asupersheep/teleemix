//! Voice feature handlers.
//!
//! Supports three Whisper backends (priority order):
//!   1. OpenAI remote API  — set OPENAI_API_KEY
//!   2. Local compatible   — set WHISPER_URL (e.g. faster-whisper-server)
//!   3. Disabled           — leave both empty
//!
//! AudD song recognition  — set AUDD_API_KEY

use reqwest::multipart;

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

    Ok(RecognitionResult { title, artist, deezer_url, spotify_url, song_link })
}

/// Query the Odesli/song.link public API to find a Deezer track URL.
/// AudD includes a song_link in every recognition result; this converts it
/// to a direct Deezer link without needing to do a text search.
pub async fn lookup_deezer_via_odesli(http: &reqwest::Client, song_link: &str) -> Option<String> {
    log::info!("Odesli lookup: {}", song_link);

    let resp = http
        .get("https://api.song.link/v1-alpha.1/links")
        .query(&[("url", song_link)])
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        log::info!("Odesli HTTP error: {}", resp.status());
        return None;
    }

    let data: serde_json::Value = resp.json().await.ok()?;
    let deezer_url = data["linksByPlatform"]["deezer"]["url"]
        .as_str()
        .map(|s| s.to_string());

    log::info!("Odesli deezer result: {:?}", deezer_url);
    deezer_url
}
