use std::env;
use std::sync::Arc;

use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use teloxide::{
    dispatching::{dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup, ParseMode},
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
    pub registered_users_file: String,
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
            registered_users_file: env::var("REGISTERED_USERS_FILE")
                .unwrap_or_else(|_| "/app/registered_users.txt".to_string()),
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
    #[command(description = "Register to receive bot notifications")]
    Register,
    #[command(description = "Show quick action buttons")]
    Menu,
    #[command(description = "Update Deezer ARL")]
    Updatearl,
}

// ── URL Patterns ──────────────────────────────────────────────────────────────


fn load_registered_users(path: &str) -> Vec<i64> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse::<i64>().ok())
        .collect()
}

fn save_registered_user(path: &str, chat_id: i64) {
    let mut users = load_registered_users(path);
    if !users.contains(&chat_id) {
        users.push(chat_id);
        let content = users.iter().map(|u| u.to_string()).collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(path, content);
    }
}

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

    // Send startup notification to all registered users
    {
        let startup_msg = "🎵 Teleemix is ready!\n\nJust send me any song name, Deezer URL, or Spotify link and I'll handle it.\n\n📲 Tap /menu for quick action buttons.";
        let registered = load_registered_users(&state.config.registered_users_file);
        for chat_id in registered {
            let _ = bot.send_message(
                teloxide::types::ChatId(chat_id),
                startup_msg,
            ).await;
        }
    }

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
        Command::Start => {
            bot.send_message(
                msg.chat.id,
                "👋 Hey! I'm Teleemix — your personal music download assistant.\n\nJust send me a song name, a Deezer link, or a Spotify link and I'll find it and queue it for download on your server. No technical stuff needed!\n\n📲 Use /menu to see quick action buttons.\n🔔 Use /register so I can notify you when I restart.\n\nFor a full list of what I can do, type /help.",
            )
            .await?;
        }

        Command::Help => {
            bot.send_message(
                msg.chat.id,
                "ℹ️ Teleemix — Full Guide\n\n🎵 What I do:\nI connect to your personal deemix server and queue music downloads for you. Just tell me what you want!\n\n📥 Ways to request music:\n• Type any song or artist name → I search and show results to pick from\n• Send a Deezer link (track, album, playlist) → queued instantly\n• Send a Spotify link (track or album) → I find it on Deezer and queue it\n\n🔧 All commands:\n/menu — button menu for quick actions\n/search — search for a track by name\n/album — search for an album by name\n/dl — queue from a Deezer URL\n/sp — queue from a Spotify link\n/status — check if deemix is online and queue size\n/register — get notified when the bot restarts\n/updatearl — update the Deezer login token when it expires\n\n💡 Tip: You don't need commands at all — just send a song name or link!",
            )
            .await?;
        }

        Command::Status => {
            match deemix::get_queue(&state).await {
                Ok(q) => {
                    let mut msg_text = "✅ Deemix is reachable\n".to_string();
                    if q.downloading > 0 {
                        msg_text.push_str(&format!("⬇️ Downloading: {}\n", q.downloading));
                    }
                    if q.pending > 0 {
                        msg_text.push_str(&format!("⏳ Pending: {}\n", q.pending));
                    }
                    if q.done > 0 {
                        msg_text.push_str(&format!("✅ Completed (in queue): {}", q.done));
                    }
                    if q.downloading == 0 && q.pending == 0 && q.done == 0 {
                        msg_text.push_str("📭 Queue is empty");
                    }
                    bot.send_message(msg.chat.id, msg_text).await?;
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

        Command::Register => {
            let chat_id = msg.chat.id.0;
            let path = state.config.registered_users_file.clone();
            save_registered_user(&path, chat_id);
            bot.send_message(msg.chat.id, "✅ Registered! You will receive notifications when the bot restarts.").await?;
        }

        Command::Menu => {
            let keyboard = KeyboardMarkup::new(vec![
                vec![
                    KeyboardButton::new("🔍 Search a track"),
                    KeyboardButton::new("💿 Search an album"),
                ],
                vec![
                    KeyboardButton::new("🟢 From Spotify link"),
                    KeyboardButton::new("🎵 From Deezer URL"),
                ],
                vec![
                    KeyboardButton::new("📊 Check deemix status"),
                    KeyboardButton::new("🔑 Update ARL"),
                ],
                vec![
                    KeyboardButton::new("🔔 Register for notifications"),
                    KeyboardButton::new("ℹ️ Help"),
                ],
            ])
            .resize_keyboard(true);

            bot.send_message(msg.chat.id, "Choose an action:")
                .reply_markup(keyboard)
                .await?;
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
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    let user_id = msg.from().map(|u| u.id.0).unwrap_or(0);
    if !state.config.is_allowed(user_id) {
        return Ok(());
    }

    let text = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => return Ok(()),
    };

    // Keyboard button presses
    match text.as_str() {
        "🔍 Search a track" => {
            dialogue.update(State::AwaitingSearch).await.ok();
            bot.send_message(msg.chat.id, "🔍 What song or artist are you looking for?").await?;
            return Ok(());
        }
        "💿 Search an album" => {
            dialogue.update(State::AwaitingAlbum).await.ok();
            bot.send_message(msg.chat.id, "💿 What album are you looking for?").await?;
            return Ok(());
        }
        "🟢 From Spotify link" => {
            dialogue.update(State::AwaitingSpotify).await.ok();
            bot.send_message(msg.chat.id, "🟢 Send me a Spotify track or album link:").await?;
            return Ok(());
        }
        "🎵 From Deezer URL" => {
            dialogue.update(State::AwaitingDl).await.ok();
            bot.send_message(msg.chat.id, "🎵 Send me a Deezer URL:").await?;
            return Ok(());
        }
        "📊 Check deemix status" => {
            match deemix::get_queue(&state).await {
                Ok(q) => {
                    let mut msg_text = "✅ Deemix is reachable
".to_string();
                    if q.downloading > 0 { msg_text.push_str(&format!("⬇️ Downloading: {}
", q.downloading)); }
                    if q.pending > 0 { msg_text.push_str(&format!("⏳ Pending: {}
", q.pending)); }
                    if q.done > 0 { msg_text.push_str(&format!("✅ Completed (in queue): {}", q.done)); }
                    if q.downloading == 0 && q.pending == 0 && q.done == 0 { msg_text.push_str("📭 Queue is empty"); }
                    bot.send_message(msg.chat.id, msg_text).await?;
                }
                Err(e) => {
                    bot.send_message(msg.chat.id, format!("❌ Can't reach deemix: {}", e)).await?;
                }
            }
            return Ok(());
        }
        "🔑 Update ARL" => {
            dialogue.update(State::AwaitingArl).await.ok();
            bot.send_message(msg.chat.id, "Please send your new Deezer ARL:").await?;
            return Ok(());
        }
        "🔔 Register for notifications" => {
            let chat_id = msg.chat.id.0;
            let path = state.config.registered_users_file.clone();
            save_registered_user(&path, chat_id);
            bot.send_message(msg.chat.id, "✅ Registered! You will receive a notification when the bot restarts.").await?;
            return Ok(());
        }
        "ℹ️ Help" => {
            bot.send_message(
                msg.chat.id,
                "ℹ️ Teleemix — Full Guide\n\n🎵 What I do:\nI connect to your personal deemix server and queue music downloads for you. Just tell me what you want!\n\n📥 Ways to request music:\n• Type any song or artist name → I search and show results to pick from\n• Send a Deezer link (track, album, playlist) → queued instantly\n• Send a Spotify link (track or album) → I find it on Deezer and queue it\n\n🔧 All commands:\n/menu — button menu for quick actions\n/search — search for a track by name\n/album — search for an album by name\n/dl — queue from a Deezer URL\n/sp — queue from a Spotify link\n/status — check if deemix is online and queue size\n/register — get notified when the bot restarts\n/updatearl — update the Deezer login token when it expires\n\n💡 Tip: You don't need commands at all — just send a song name or link!",
            ).await?;
            return Ok(());
        }
        _ => {}
    }

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
