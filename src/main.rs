use std::env;
use std::sync::Arc;

use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use teloxide::{
    dispatching::{dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode},
    utils::command::BotCommands,
};

mod spotify;
mod deemix;

// ── Dialogue State ────────────────────────────────────────────────────────────

#[derive(Clone, Default, Debug)]
pub enum State {
    #[default]
    Idle,
    AwaitingArl,
    AwaitingSearch,
    AwaitingAlbum,
    AwaitingDl,
    AwaitingSpotify,
}

type MyDialogue = Dialogue<State, InMemStorage<State>>;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Config {
    pub deemix_url: String,
    pub deemix_arl: String,
    pub allowed_users: Vec<u64>,
    pub compose_file: String,
    pub service_name: String,
    pub env_file: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            deemix_url: env::var("DEEMIX_URL")
                .unwrap_or_else(|_| "http://localhost:6595".to_string()),
            deemix_arl: env::var("DEEMIX_ARL").unwrap_or_default(),
            allowed_users: env::var("ALLOWED_USERS")
                .unwrap_or_default()
                .split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect(),
            compose_file: env::var("COMPOSE_FILE")
                .unwrap_or_else(|_| "/compose/docker-compose.yml".to_string()),
            service_name: env::var("SERVICE_NAME")
                .unwrap_or_else(|_| "teleemix".to_string()),
            env_file: env::var("ENV_FILE")
                .unwrap_or_else(|_| "/app/.env".to_string()),
        }
    }

    pub fn is_allowed(&self, user_id: u64) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.contains(&user_id)
    }
}

// ── Bot State ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BotState {
    pub config: Arc<Config>,
    pub http: Client,
}

impl BotState {
    pub fn new(config: Config) -> Self {
        let http = Client::builder()
            .cookie_store(true)  // persist connect.sid across requests
            .build()
            .expect("Failed to build HTTP client");
        Self {
            config: Arc::new(config),
            http,
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Teleemix commands:")]
enum Command {
    #[command(description = "Show help")]
    Start,
    #[command(description = "Show help")]
    Help,
    #[command(description = "Check deemix status")]
    Status,
    #[command(description = "Queue a Deezer URL")]
    Dl,
    #[command(description = "Search for a track")]
    Search,
    #[command(description = "Search for an album")]
    Album,
    #[command(description = "Download from a Spotify link")]
    Sp,
    #[command(description = "Update Deezer ARL")]
    Updatearl,
}

// ── URL Patterns ──────────────────────────────────────────────────────────────

lazy_static::lazy_static! {
    static ref DEEZER_URL_RE: Regex = Regex::new(
        r"https?://(?:www\.)?deezer\.com/(?:[a-z]+/)?(track|album|playlist|artist)/(\d+)"
    ).unwrap();
    static ref DEEZER_SHORT_RE: Regex = Regex::new(
        r"https?://link\.deezer\.com/s/\S+"
    ).unwrap();
    static ref SPOTIFY_TRACK_RE: Regex = Regex::new(
        r"https?://open\.spotify\.com/track/([A-Za-z0-9]+)"
    ).unwrap();
    static ref SPOTIFY_ALBUM_RE: Regex = Regex::new(
        r"https?://open\.spotify\.com/album/([A-Za-z0-9]+)"
    ).unwrap();
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    let config = Config::from_env();
    let state = Arc::new(BotState::new(config));

    let token = env::var("TELEGRAM_TOKEN").expect("TELEGRAM_TOKEN must be set");
    let bot = Bot::new(token);

    // Login to deemix on startup
    deemix::login(&state).await;

    log::info!("Teleemix bot starting...");

    let storage = InMemStorage::<State>::new();

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .enter_dialogue::<Message, InMemStorage<State>, State>()
                .branch(dptree::case![State::AwaitingArl].endpoint(receive_arl))
                .branch(dptree::case![State::AwaitingSearch].endpoint(receive_search))
                .branch(dptree::case![State::AwaitingAlbum].endpoint(receive_album))
                .branch(dptree::case![State::AwaitingDl].endpoint(receive_dl))
                .branch(dptree::case![State::AwaitingSpotify].endpoint(receive_spotify))
                .branch(
                    Update::filter_message()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(
                    Update::filter_message()
                        .endpoint(handle_message),
                ),
        )
        .branch(
            Update::filter_callback_query()
                .endpoint(handle_callback),
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![Arc::clone(&state), storage])
        .build()
        .dispatch()
        .await;
}

// ── Command Handler ───────────────────────────────────────────────────────────

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    let user_id = msg.from().map(|u| u.id.0).unwrap_or(0);
    if !state.config.is_allowed(user_id) {
        bot.send_message(msg.chat.id, "⛔ Not authorised.").await?;
        return Ok(());
    }

    match cmd {
        Command::Start | Command::Help => {
            bot.send_message(
                msg.chat.id,
                "🎵 Teleemix\n\nSend me any of these:\n• A Deezer URL — queued instantly\n• A Spotify track/album link — looked up and queued\n• Any song or artist name — search and pick\n\nCommands:\n/dl <deezer url> — queue a download\n/search <song> — search tracks\n/album <name> — search albums\n/status — check deemix\n/updatearl — update your Deezer ARL (interactive)",
            )
            .await?;
        }

        Command::Status => {
            match deemix::get_queue(&state).await {
                Ok(count) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("✅ Deemix is reachable\n📥 Items in queue: {}", count),
                    )
                    .await?;
                }
                Err(e) => {
                    bot.send_message(msg.chat.id, format!("❌ Can't reach deemix: {}", e))
                        .await?;
                }
            }
        }

        Command::Dl => {
            dialogue.update(State::AwaitingDl).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "🎵 Send me a Deezer URL:").await?;
        }

        Command::Search => {
            dialogue.update(State::AwaitingSearch).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "🔍 What song or artist are you looking for?").await?;
        }

        Command::Album => {
            dialogue.update(State::AwaitingAlbum).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "💿 What album are you looking for?").await?;
        }

        Command::Sp => {
            dialogue.update(State::AwaitingSpotify).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "🟢 Send me a Spotify track or album link:").await?;
        }

        Command::Updatearl => {
            if !state.config.is_allowed(msg.from().map(|u| u.id.0).unwrap_or(0)) {
                bot.send_message(msg.chat.id, "⛔ Not authorised.").await?;
                return Ok(());
            }
            dialogue.update(State::AwaitingArl).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "Please send your new Deezer ARL:").await?;
        }
    }

    Ok(())
}

// ── Message Handler ───────────────────────────────────────────────────────────



async fn receive_search(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(query) = msg.text() {
        do_search(&bot, &msg, &state, query.trim(), "track").await?;
    }
    Ok(())
}

async fn receive_album(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(query) = msg.text() {
        do_search(&bot, &msg, &state, query.trim(), "album").await?;
    }
    Ok(())
}

async fn receive_dl(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(url) = msg.text() {
        queue_url(&bot, &msg, &state, url.trim()).await?;
    }
    Ok(())
}

async fn receive_spotify(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(url) = msg.text() {
        handle_spotify(&bot, &msg, &state, url.trim()).await?;
    }
    Ok(())
}

async fn receive_arl(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    let user_id = msg.from().map(|u| u.id.0).unwrap_or(0);
    if !state.config.is_allowed(user_id) {
        dialogue.exit().await.ok();
        return Ok(());
    }

    let arl = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => {
            bot.send_message(msg.chat.id, "Please send the ARL as text.").await?;
            return Ok(());
        }
    };

    if arl.len() < 100 {
        bot.send_message(msg.chat.id, "❌ That ARL looks too short, double check it. Try again:").await?;
        return Ok(());
    }

    dialogue.exit().await.ok();
    handle_updatearl(&bot, &msg, &state, &arl).await?;
    Ok(())
}

async fn handle_message(
    bot: Bot,
    msg: Message,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    let user_id = msg.from().map(|u| u.id.0).unwrap_or(0);
    if !state.config.is_allowed(user_id) {
        return Ok(());
    }

    let text = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => return Ok(()),
    };

    // Spotify URL
    if SPOTIFY_TRACK_RE.is_match(&text) || SPOTIFY_ALBUM_RE.is_match(&text) {
        handle_spotify(&bot, &msg, &state, &text).await?;
        return Ok(());
    }

    // Deezer short URL
    if DEEZER_SHORT_RE.is_match(&text) {
        let resolved = resolve_short_link(&state.http, &text).await;
        if let Some(url) = resolved {
            queue_url(&bot, &msg, &state, &url).await?;
        } else {
            bot.send_message(msg.chat.id, "❌ Could not resolve that link.").await?;
        }
        return Ok(());
    }

    // Full Deezer URL
    if DEEZER_URL_RE.is_match(&text) {
        queue_url(&bot, &msg, &state, &text).await?;
        return Ok(());
    }

    // Plain text — search
    do_search(&bot, &msg, &state, &text, "track").await?;
    Ok(())
}

// ── Callback Handler ──────────────────────────────────────────────────────────

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    state: Arc<BotState>,
) -> ResponseResult<()> {
    bot.answer_callback_query(&q.id).await?;

    let data = match &q.data {
        Some(d) => d.clone(),
        None => return Ok(()),
    };

    if data == "cancel" {
        if let Some(msg) = &q.message {
            bot.edit_message_text(msg.chat.id, msg.id, "Cancelled.").await?;
        }
        return Ok(());
    }

    if let Some(url) = data.strip_prefix("dl:") {
        if let Some(msg) = &q.message {
            bot.edit_message_text(msg.chat.id, msg.id, "⏳ Queuing download...").await?;
            match deemix::add_to_queue(&state, url).await {
                Ok(_) => {
                    bot.edit_message_text(
                        msg.chat.id,
                        msg.id,
                        format!("✅ Added to queue!\n{}", url),
                    )
                    .await?;
                }
                Err(e) => {
                    bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Failed: {}", e))
                        .await?;
                }
            }
        }
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn resolve_short_link(http: &Client, url: &str) -> Option<String> {
    let resp = http.head(url).send().await.ok()?;
    Some(resp.url().to_string())
}

async fn queue_url(
    bot: &Bot,
    msg: &Message,
    state: &Arc<BotState>,
    url: &str,
) -> ResponseResult<()> {
    let cap = match DEEZER_URL_RE.captures(url) {
        Some(c) => c,
        None => {
            bot.send_message(msg.chat.id, "❌ That doesn't look like a valid Deezer URL.")
                .await?;
            return Ok(());
        }
    };
    let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("item");
    let sent = bot
        .send_message(msg.chat.id, format!("⏳ Queuing {}...", kind))
        .await?;

    match deemix::add_to_queue(state, url).await {
        Ok(_) => {
            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                format!("✅ {} added to queue!", capitalize(kind)),
            )
            .await?;
        }
        Err(e) => {
            bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Failed to queue: {}", e))
                .await?;
        }
    }
    Ok(())
}

async fn do_search(
    bot: &Bot,
    msg: &Message,
    state: &Arc<BotState>,
    query: &str,
    search_type: &str,
) -> ResponseResult<()> {
    let sent = bot
        .send_message(
            msg.chat.id,
            format!("🔍 Searching for {}...", query),
        )
        .await?;

    match deemix::search(state, query, search_type).await {
        Ok(results) if results.is_empty() => {
            bot.edit_message_text(msg.chat.id, sent.id, "😕 No results found.")
                .await?;
        }
        Ok(results) => {
            let mut buttons: Vec<Vec<InlineKeyboardButton>> = results
                .iter()
                .map(|item| {
                    let label = if search_type == "track" {
                        format!(
                            "🎵 {} — {}",
                            item["title"].as_str().unwrap_or("?"),
                            item["artist"]["name"].as_str().unwrap_or("?")
                        )
                    } else {
                        format!(
                            "💿 {} — {}",
                            item["title"].as_str().unwrap_or("?"),
                            item["artist"]["name"].as_str().unwrap_or("?")
                        )
                    };
                    let label = if label.len() > 60 {
                        format!("{}...", &label[..57])
                    } else {
                        label
                    };
                    let url = item["link"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    vec![InlineKeyboardButton::callback(label, format!("dl:{}", url))]
                })
                .collect();
            buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);

            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                format!("Results for {}:", query),
            )
            .reply_markup(InlineKeyboardMarkup::new(buttons))
            .await?;
        }
        Err(e) => {
            bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Search failed: {}", e))
                .await?;
        }
    }
    Ok(())
}

async fn handle_spotify(
    bot: &Bot,
    msg: &Message,
    state: &Arc<BotState>,
    url: &str,
) -> ResponseResult<()> {
    let sent = bot
        .send_message(msg.chat.id, "🎵 Looking up Spotify link...")
        .await?;

    let search_type = if SPOTIFY_TRACK_RE.is_match(url) {
        "track"
    } else {
        "album"
    };

    match spotify::resolve(url).await {
        Some(meta) => {
            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                format!("🔍 Found {}\nSearching on Deezer...", meta.label),
            )
            .await?;

            // Re-use do_search but we need a fresh message — edit the existing one
            match deemix::search(state, &meta.query, search_type).await {
                Ok(results) if results.is_empty() => {
                    bot.edit_message_text(
                        msg.chat.id,
                        sent.id,
                        format!("😕 No results found on Deezer for: {}", meta.query),
                    )
                    .await?;
                }
                Ok(results) => {
                    let mut buttons: Vec<Vec<InlineKeyboardButton>> = results
                        .iter()
                        .map(|item| {
                            let label = format!(
                                "🎵 {} — {}",
                                item["title"].as_str().unwrap_or("?"),
                                item["artist"]["name"].as_str().unwrap_or("?")
                            );
                            let label = if label.len() > 60 {
                                format!("{}...", &label[..57])
                            } else {
                                label
                            };
                            let url = item["link"].as_str().unwrap_or("").to_string();
                            vec![InlineKeyboardButton::callback(label, format!("dl:{}", url))]
                        })
                        .collect();
                    buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);

                    bot.edit_message_text(
                        msg.chat.id,
                        sent.id,
                        format!("Results for {}:", meta.query),
                    )
                    .reply_markup(InlineKeyboardMarkup::new(buttons))
                    .await?;
                }
                Err(e) => {
                    bot.edit_message_text(
                        msg.chat.id,
                        sent.id,
                        format!("❌ Search failed: {}", e),
                    )
                    .await?;
                }
            }
        }
        None => {
            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                "❌ Could not resolve Spotify link. Try `/search` instead.",
            )
            .await?;
        }
    }
    Ok(())
}

async fn handle_updatearl(
    bot: &Bot,
    msg: &Message,
    state: &Arc<BotState>,
    arl: &str,
) -> ResponseResult<()> {
    let sent = bot.send_message(msg.chat.id, "🔄 Validating new ARL...").await?;

    // Login with new ARL
    match deemix::login_arl(state, arl).await {
        Ok(username) => {
            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                format!("✅ Logged in as {}\n🔄 Updating compose file...", username),
            )
            .await?;

            // Update .env file
            let env_path = &state.config.env_file;
            match std::fs::read_to_string(env_path) {
                Ok(contents) => {
                    let updated = regex::Regex::new(r"DEEMIX_ARL=.*")
                        .unwrap()
                        .replace(&contents, format!("DEEMIX_ARL={}", arl))
                        .to_string();
                    if let Err(e) = std::fs::write(env_path, &updated) {
                        bot.edit_message_text(
                            msg.chat.id,
                            sent.id,
                            format!("⚠️ Logged in OK but could not update .env file: {}", e),
                        )
                        .await?;
                        return Ok(());
                    }
                }
                Err(e) => {
                    bot.edit_message_text(
                        msg.chat.id,
                        sent.id,
                        format!("⚠️ Logged in OK but could not read .env file: {}", e),
                    )
                    .await?;
                    return Ok(());
                }
            }

            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                "✅ ARL updated and saved! Downloads will use the new ARL immediately.",
            )
            .await?;

            // Trigger rebuild detached
            // No restart needed — bot already re-logged into deemix in memory above.
            // .env file was updated so the new ARL persists across future restarts.
        }
        Err(e) => {
            bot.edit_message_text(
                msg.chat.id,
                sent.id,
                format!("❌ ARL rejected by deemix: {}", e),
            )
            .await?;
        }
    }
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn escape_md(s: &str) -> String {
    s.replace('.', "\\.")
        .replace('!', "\\!")
        .replace('-', "\\-")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('>', "\\>")
        .replace('#', "\\#")
        .replace('+', "\\+")
        .replace('=', "\\=")
        .replace('|', "\\|")
        .replace('~', "\\~")
        .replace('`', "\\`")
}
