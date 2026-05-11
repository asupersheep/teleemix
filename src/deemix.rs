use std::sync::Arc;

use serde_json::Value;

use crate::BotState;

pub async fn login(state: &Arc<BotState>) {
    if state.config.deemix_arl.is_empty() {
        log::warn!("DEEMIX_ARL not set — bot may get NotLoggedIn errors.");
        return;
    }
    match login_arl(state, &state.config.deemix_arl.clone()).await {
        Ok(_) => log::info!("Successfully logged into deemix"),
        Err(e) => log::warn!("Could not login to deemix: {}", e),
    }
}

pub async fn login_arl(state: &Arc<BotState>, arl: &str) -> Result<String, String> {
    let url = format!("{}/api/loginArl", state.config.deemix_url);
    let resp = state
        .http
        .post(&url)
        .json(&serde_json::json!({ "arl": arl }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;

    // status 1 = logged in, status 2 = already logged in
    let status = data["status"].as_i64().unwrap_or(0);
    if status == 1 || status == 2 {
        let username = data["user"]["name"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        Ok(username)
    } else {
        Err(format!("Login failed (status {})", status))
    }
}

pub async fn add_to_queue(state: &Arc<BotState>, url: &str) -> Result<(), String> {
    let endpoint = format!("{}/api/addToQueue", state.config.deemix_url);
    let resp = state
        .http
        .post(&endpoint)
        .json(&serde_json::json!({ "url": url, "bitrate": 9 }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;

    if data["result"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(data["errid"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string())
    }
}

pub struct QueueStatus {
    pub pending: usize,
    pub downloading: usize,
    pub done: usize,
}

pub async fn get_queue(state: &Arc<BotState>) -> Result<QueueStatus, String> {
    let url = format!("{}/api/getQueue", state.config.deemix_url);
    let resp = state
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut pending = 0usize;
    let mut downloading = 0usize;
    let mut done = 0usize;

    if let Some(queue) = data["queue"].as_object() {
        for item in queue.values() {
            let progress = item["progress"].as_u64().unwrap_or(0);
            let downloaded = item["downloaded"].as_u64().unwrap_or(0);
            let size = item["size"].as_u64().unwrap_or(1);
            if downloaded >= size && size > 0 {
                done += 1;
            } else if progress > 0 {
                downloading += 1;
            } else {
                pending += 1;
            }
        }
    }

    Ok(QueueStatus { pending, downloading, done })
}

pub async fn search(
    state: &Arc<BotState>,
    query: &str,
    search_type: &str,
) -> Result<Vec<Value>, String> {
    let url = format!("{}/api/search", state.config.deemix_url);
    let resp = state
        .http
        .get(&url)
        .query(&[("term", query), ("type", search_type)])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: Value = resp.json().await.map_err(|e| e.to_string())?;

    let results = data["data"]
        .as_array()
        .or_else(|| {
            data["results"]["data"].as_array()
        })
        .cloned()
        .unwrap_or_default();

    Ok(results.into_iter().take(8).collect())
}
