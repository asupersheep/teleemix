use std::env;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;

use regex::Regex;
use reqwest::Client;
use teloxide::{
    dispatching::dialogue::InMemStorage,
    prelude::*,
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup,
        KeyboardButton, KeyboardMarkup,
    },
    utils::command::BotCommands,
};

mod spotify;
mod deemix;
mod users;
mod voice;

use users::{UsersDb, UserSettings};

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
    pub compose_file: String,
    pub service_name: String,
    pub env_file: String,
    pub users_file: String,
    pub audd_api_key: String,
    pub openai_api_key: String,
    pub whisper_url: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            deemix_url: env::var("DEEMIX_URL")
                .unwrap_or_else(|_| "http://localhost:6595".to_string()),
            deemix_arl: env::var("DEEMIX_ARL").unwrap_or_default(),
            compose_file: env::var("COMPOSE_FILE")
                .unwrap_or_else(|_| "/compose/docker-compose.yml".to_string()),
            service_name: env::var("SERVICE_NAME")
                .unwrap_or_else(|_| "teleemix".to_string()),
            env_file: env::var("ENV_FILE")
                .unwrap_or_else(|_| "/app/.env".to_string()),
            users_file: env::var("USERS_FILE")
                .unwrap_or_else(|_| "/app/users.json".to_string()),
            audd_api_key: env::var("AUDD_API_KEY").unwrap_or_default(),
            openai_api_key: env::var("OPENAI_API_KEY").unwrap_or_default(),
            whisper_url: env::var("WHISPER_URL").unwrap_or_default(),
        }
    }


    pub fn audd_enabled(&self) -> bool { !self.audd_api_key.is_empty() }
    pub fn whisper_enabled(&self) -> bool { !self.openai_api_key.is_empty() || !self.whisper_url.is_empty() }
}

// ── Bot State ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BotState {
    pub config: Arc<Config>,
    pub http: Client,
    pub users: UsersDb,
    pub pending_voices: Arc<Mutex<HashMap<String, String>>>, // short_id -> file_id
}

impl BotState {
    pub fn new(config: Config, users: UsersDb) -> Self {
        let http = Client::builder()
            .cookie_store(true)
            .build()
            .expect("Failed to build HTTP client");
        Self {
            config: Arc::new(config),
            http,
            users,
            pending_voices: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Teleemix commands:")]
enum Command {
    #[command(description = "Welcome message")]
    Start,
    #[command(description = "Show all commands and info")]
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
    #[command(description = "Show quick action buttons")]
    Menu,
    #[command(description = "Show settings")]
    Settings,
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
    let users = users::load(&config.users_file);
    let state = Arc::new(BotState::new(config, users));

    let token = env::var("TELEGRAM_TOKEN").expect("TELEGRAM_TOKEN must be set");
    let bot = Bot::new(token);

    deemix::login(&state).await;

    log::info!("Teleemix bot starting...");

    // Send startup notification to users with restart_notifications enabled
    {
        let startup_msg = "🎵 Teleemix is back online!\n\nTap /menu for quick actions.";
        for chat_id in users::all_with_notifications(&state.users) {
            let _ = bot.send_message(teloxide::types::ChatId(chat_id), startup_msg).await;
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

// ── Helpers ───────────────────────────────────────────────────────────────────

fn user_id_from_msg(msg: &Message) -> i64 {
    msg.from().map(|u| u.id.0 as i64).unwrap_or(0)
}

fn settings_keyboard(s: &UserSettings, config: &Config) -> KeyboardMarkup {
    let notif = if s.restart_notifications { "🔔 Restart notifications: ON" } else { "🔕 Restart notifications: OFF" };
    let voice = if s.voice_search && config.whisper_enabled() { "🎤 Voice search: ON" } else { "🎤 Voice search: OFF" };
    let recog = if s.song_recognition && config.audd_enabled() { "🎵 Song recognition: ON" } else { "🎵 Song recognition: OFF" };

    KeyboardMarkup::new(vec![
        vec![KeyboardButton::new(notif)],
        vec![KeyboardButton::new(voice)],
        vec![KeyboardButton::new(recog)],
        vec![KeyboardButton::new("🔑 Update ARL")],
        vec![KeyboardButton::new("🔙 Back to menu")],
    ])
    .resize_keyboard(true)
}

fn main_keyboard(s: &UserSettings, config: &Config) -> KeyboardMarkup {
    let mut rows = vec![
        vec![
            KeyboardButton::new("🔍 Search a track"),
            KeyboardButton::new("💿 Search an album"),
        ],
        vec![
            KeyboardButton::new("🟢 From Spotify link"),
            KeyboardButton::new("🎵 From Deezer URL"),
        ],
    ];

    // Only show voice buttons if features are configured AND user has them enabled
    let show_voice = config.whisper_enabled() && s.voice_search;
    let show_recog = config.audd_enabled() && s.song_recognition;

    if show_voice || show_recog {
        let mut voice_row = vec![];
        if show_voice { voice_row.push(KeyboardButton::new("🎤 Voice search")); }
        if show_recog { voice_row.push(KeyboardButton::new("🎵 Recognize song")); }
        rows.push(voice_row);
    }

    rows.push(vec![
        KeyboardButton::new("📊 Check status"),
        KeyboardButton::new("⚙️ Settings"),
    ]);
    rows.push(vec![KeyboardButton::new("ℹ️ Help")]);

    KeyboardMarkup::new(rows).resize_keyboard(true)
}

// ── Command Handler ───────────────────────────────────────────────────────────

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    state: Arc<BotState>,
    dialogue: MyDialogue,
) -> ResponseResult<()> {
    // Auto-create user on any interaction
    let user_settings = users::get_or_create(&state.users, user_id_from_msg(&msg));
    users::save(&state.users, &state.config.users_file);

    match cmd {
        Command::Start => {
            let kb = main_keyboard(&user_settings, &state.config);
            bot.send_message(
                msg.chat.id,
                "👋 Hey! I'm Teleemix — your personal music download assistant.\n\nJust send me a song name, a Deezer link, or a Spotify link and I'll find it and queue it for download on your server. No technical stuff needed!\n\n📲 Use /menu to see quick action buttons.\n\nFor a full list of what I can do, type /help.",
            )
            .reply_markup(kb)
            .await?;
        }

        Command::Help => {
            bot.send_message(
                msg.chat.id,
                "ℹ️ Teleemix — Full Guide\n\n\
🎵 What I do:\n\
I connect to your personal deemix server and queue music downloads for you. Just tell me what you want!\n\n\
📥 Ways to request music:\n\
• Type any song or artist name → search and pick from results\n\
• Send a Deezer link (track, album, playlist) → queued instantly\n\
• Send a Spotify link (track or album) → found on Deezer and queued\n\
• Send a voice note → transcribe what you said or recognize the song\n\n\
🔧 All commands:\n\
/menu — quick action buttons\n\
/search — search for a track\n\
/album — search for an album\n\
/dl — queue from a Deezer URL\n\
/sp — queue from a Spotify link\n\
/status — check deemix connection and queue\n\
/settings — manage your personal preferences\n\
/updatearl — update your Deezer ARL\n\n\
⚙️ Settings (via /settings):\n\
• Restart notifications — get notified when the bot restarts\n\
• Voice search — transcribe voice notes to search\n\
• Song recognition — identify songs from voice recordings\n\n\
💡 Tip: You don't need commands — just send a song name or link directly!",
            )
            .await?;
        }

        Command::Status => {
            match deemix::get_queue(&state).await {
                Ok(q) => {
                    let mut text = "✅ Deemix is reachable\n".to_string();
                    if q.downloading > 0 { text.push_str(&format!("⬇️ Downloading: {}\n", q.downloading)); }
                    if q.pending > 0 { text.push_str(&format!("⏳ Pending: {}\n", q.pending)); }
                    if q.done > 0 { text.push_str(&format!("✅ Completed (in queue): {}", q.done)); }
                    if q.downloading == 0 && q.pending == 0 && q.done == 0 { text.push_str("📭 Queue is empty"); }
                    bot.send_message(msg.chat.id, text).await?;
                }
                Err(e) => { bot.send_message(msg.chat.id, format!("❌ Can't reach deemix: {}", e)).await?; }
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

        Command::Menu => {
            let kb = main_keyboard(&user_settings, &state.config);
            bot.send_message(msg.chat.id, "Choose an action:").reply_markup(kb).await?;
        }

        Command::Settings => {
            let kb = settings_keyboard(&user_settings, &state.config);
            bot.send_message(msg.chat.id, "⚙️ Your settings — tap to toggle:").reply_markup(kb).await?;
        }

        Command::Updatearl => {
            dialogue.update(State::AwaitingArl).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            bot.send_message(msg.chat.id, "Please send your new Deezer ARL:").await?;
        }
    }

    Ok(())
}

// ── Dialogue Receivers ────────────────────────────────────────────────────────

async fn receive_search(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(query) = msg.text() { do_search(&bot, &msg, &state, query.trim(), "track").await?; }
    Ok(())
}

async fn receive_album(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(query) = msg.text() { do_search(&bot, &msg, &state, query.trim(), "album").await?; }
    Ok(())
}

async fn receive_dl(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(url) = msg.text() { queue_url(&bot, &msg, &state, url.trim()).await?; }
    Ok(())
}

async fn receive_spotify(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    dialogue.exit().await.ok();
    if let Some(url) = msg.text() { handle_spotify(&bot, &msg, &state, url.trim()).await?; }
    Ok(())
}

async fn receive_arl(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    let arl = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => { bot.send_message(msg.chat.id, "Please send the ARL as text.").await?; return Ok(()); }
    };

    if arl.len() < 100 {
        bot.send_message(msg.chat.id, "❌ That ARL looks too short, double check it. Try again:").await?;
        return Ok(());
    }

    dialogue.exit().await.ok();
    handle_updatearl(&bot, &msg, &state, &arl).await?;
    Ok(())
}

// ── Message Handler ───────────────────────────────────────────────────────────

async fn handle_message(bot: Bot, msg: Message, state: Arc<BotState>, dialogue: MyDialogue) -> ResponseResult<()> {
    // Auto-create user
    let user_settings = users::get_or_create(&state.users, user_id_from_msg(&msg));
    users::save(&state.users, &state.config.users_file);

    // ── Voice note handling ──
    if let Some(voice) = msg.voice() {
        dialogue.exit().await.ok(); // exit any active dialogue state
        let user_settings = users::get_or_create(&state.users, user_id_from_msg(&msg));
        let audd_on = state.config.audd_enabled() && user_settings.song_recognition;
        let whisper_on = state.config.whisper_enabled() && user_settings.voice_search;

        if !audd_on && !whisper_on {
            bot.send_message(msg.chat.id, "⚠️ Voice features are not configured or disabled. Use /settings to manage them.").await?;
            return Ok(());
        }

        // Store file_id with a short key to stay under Telegram's 64 byte callback limit
        let short_id = format!("{}", msg.id.0);
        {
            let mut map = state.pending_voices.lock().await;
            map.insert(short_id.clone(), voice.file.id.to_string());
        }

        // Build choice buttons based on what's enabled
        let mut buttons: Vec<Vec<teloxide::types::InlineKeyboardButton>> = vec![];
        if whisper_on {
            buttons.push(vec![teloxide::types::InlineKeyboardButton::callback(
                "🎤 Transcribe what I said",
                format!("vt:{}", short_id),
            )]);
        }
        if audd_on {
            buttons.push(vec![teloxide::types::InlineKeyboardButton::callback(
                "🎵 Recognize the song",
                format!("vr:{}", short_id),
            )]);
        }
        buttons.push(vec![teloxide::types::InlineKeyboardButton::callback("❌ Cancel", "cancel")]);

        bot.send_message(msg.chat.id, "🎙️ What should I do with this voice note?")
            .reply_markup(teloxide::types::InlineKeyboardMarkup::new(buttons))
            .await?;
        return Ok(());
    }

    let text = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => return Ok(()),
    };

    // ── Keyboard button presses ──
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
        "🎤 Voice search" => {
            if !state.config.whisper_enabled() || !user_settings.voice_search {
                bot.send_message(msg.chat.id, "⚠️ Voice search is not configured. Add OPENAI_API_KEY or WHISPER_URL to your .env, or enable it in /settings.").await?;
            } else {
                bot.send_message(msg.chat.id, "🎤 Send me a voice note and I'll transcribe what you said and search for it.").await?;
            }
            return Ok(());
        }
        "🎵 Recognize song" => {
            if !state.config.audd_enabled() || !user_settings.song_recognition {
                bot.send_message(msg.chat.id, "⚠️ Song recognition is not configured. Add AUDD_API_KEY to your .env, or enable it in /settings.").await?;
            } else {
                bot.send_message(msg.chat.id, "🎵 Send me a voice recording of a song and I'll identify it.").await?;
            }
            return Ok(());
        }
        "📊 Check status" => {
            match deemix::get_queue(&state).await {
                Ok(q) => {
                    let mut t = "✅ Deemix is reachable\n".to_string();
                    if q.downloading > 0 { t.push_str(&format!("⬇️ Downloading: {}\n", q.downloading)); }
                    if q.pending > 0 { t.push_str(&format!("⏳ Pending: {}\n", q.pending)); }
                    if q.done > 0 { t.push_str(&format!("✅ Completed: {}", q.done)); }
                    if q.downloading == 0 && q.pending == 0 && q.done == 0 { t.push_str("📭 Queue is empty"); }
                    bot.send_message(msg.chat.id, t).await?;
                }
                Err(e) => { bot.send_message(msg.chat.id, format!("❌ Can't reach deemix: {}", e)).await?; }
            }
            return Ok(());
        }
        "⚙️ Settings" => {
            let kb = settings_keyboard(&user_settings, &state.config);
            bot.send_message(msg.chat.id, "⚙️ Your settings — tap to toggle:").reply_markup(kb).await?;
            return Ok(());
        }
        "🔙 Back to menu" => {
            let kb = main_keyboard(&user_settings, &state.config);
            bot.send_message(msg.chat.id, "Choose an action:").reply_markup(kb).await?;
            return Ok(());
        }
        "🔑 Update ARL" => {
            dialogue.update(State::AwaitingArl).await.ok();
            bot.send_message(msg.chat.id, "Please send your new Deezer ARL:").await?;
            return Ok(());
        }
        "ℹ️ Help" => {
            bot.send_message(msg.chat.id,
                "ℹ️ Teleemix — Full Guide\n\n\
🎵 What I do:\n\
I connect to your personal deemix server and queue music downloads. Just tell me what you want!\n\n\
📥 Ways to request music:\n\
• Type any song or artist name → search and pick\n\
• Send a Deezer link → queued instantly\n\
• Send a Spotify link → found on Deezer and queued\n\
• Send a voice note → transcribe or recognize\n\n\
🔧 Commands:\n\
/menu — quick action buttons\n\
/search — search for a track\n\
/album — search for an album\n\
/dl — queue from a Deezer URL\n\
/sp — queue from a Spotify link\n\
/status — check deemix\n\
/settings — your personal settings\n\
/updatearl — update your Deezer ARL\n\n\
💡 Tip: Just send a song name or link — no commands needed!"
            ).await?;
            return Ok(());
        }
        // Settings toggles
        t if t.starts_with("🔔 Restart notifications:") || t.starts_with("🔕 Restart notifications:") => {
            users::update(&state.users, &state.config.users_file, user_id_from_msg(&msg), |s| {
                s.restart_notifications = !s.restart_notifications;
            });
            let updated = users::get_or_create(&state.users, user_id_from_msg(&msg));
            let kb = settings_keyboard(&updated, &state.config);
            let status = if updated.restart_notifications { "ON" } else { "OFF" };
            bot.send_message(msg.chat.id, format!("🔔 Restart notifications: {}", status)).reply_markup(kb).await?;
            return Ok(());
        }
        t if t.starts_with("🎤 Voice search:") => {
            if !state.config.whisper_enabled() {
                bot.send_message(msg.chat.id, "⚠️ Voice search is not configured. Add OPENAI_API_KEY to your .env to enable it.").await?;
                return Ok(());
            }
            users::update(&state.users, &state.config.users_file, user_id_from_msg(&msg), |s| {
                s.voice_search = !s.voice_search;
            });
            let updated = users::get_or_create(&state.users, user_id_from_msg(&msg));
            let kb = settings_keyboard(&updated, &state.config);
            let status = if updated.voice_search { "ON" } else { "OFF" };
            bot.send_message(msg.chat.id, format!("🎤 Voice search: {}", status)).reply_markup(kb).await?;
            return Ok(());
        }
        t if t.starts_with("🎵 Song recognition:") => {
            if !state.config.audd_enabled() {
                bot.send_message(msg.chat.id, "⚠️ Song recognition is not configured. Add AUDD_API_KEY to your .env to enable it.").await?;
                return Ok(());
            }
            users::update(&state.users, &state.config.users_file, user_id_from_msg(&msg), |s| {
                s.song_recognition = !s.song_recognition;
            });
            let updated = users::get_or_create(&state.users, user_id_from_msg(&msg));
            let kb = settings_keyboard(&updated, &state.config);
            let status = if updated.song_recognition { "ON" } else { "OFF" };
            bot.send_message(msg.chat.id, format!("🎵 Song recognition: {}", status)).reply_markup(kb).await?;
            return Ok(());
        }
        _ => {}
    }

    // ── Voice notes ──
    if let Some(voice) = msg.voice() {
        handle_voice_note(&bot, &msg, &state, voice.file.id.clone()).await?;
        return Ok(());
    }

    // ── Spotify URL ──
    if SPOTIFY_TRACK_RE.is_match(&text) || SPOTIFY_ALBUM_RE.is_match(&text) {
        handle_spotify(&bot, &msg, &state, &text).await?;
        return Ok(());
    }

    // ── Deezer short URL ──
    if DEEZER_SHORT_RE.is_match(&text) {
        let resolved = resolve_short_link(&state.http, &text).await;
        if let Some(url) = resolved {
            queue_url(&bot, &msg, &state, &url).await?;
        } else {
            bot.send_message(msg.chat.id, "❌ Could not resolve that link.").await?;
        }
        return Ok(());
    }

    // ── Full Deezer URL ──
    if DEEZER_URL_RE.is_match(&text) {
        queue_url(&bot, &msg, &state, &text).await?;
        return Ok(());
    }

    // ── Plain text search ──
    do_search(&bot, &msg, &state, &text, "track").await?;
    Ok(())
}

// ── Callback Handler ──────────────────────────────────────────────────────────

async fn handle_callback(bot: Bot, q: CallbackQuery, state: Arc<BotState>) -> ResponseResult<()> {
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
                Ok(_) => { bot.edit_message_text(msg.chat.id, msg.id, format!("✅ Added to queue!\n{}", url)).await?; }
                Err(e) => { bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Failed: {}", e)).await?; }
            }
        }
    }

    // ── Voice callbacks ──
    let is_transcribe = data.starts_with("vt:");
    let is_recognize = data.starts_with("vr:");
    if is_transcribe || is_recognize {
        if let Some(msg) = &q.message {
            let short_id = &data[3..];
            let action = if is_transcribe { "transcribe" } else { "recognize" };

            // Retrieve file_id from pending_voices map
            let file_id = {
                let map = state.pending_voices.lock().await;
                map.get(short_id).cloned()
            };
            let file_id = match file_id {
                Some(f) => f,
                None => {
                    bot.edit_message_text(msg.chat.id, msg.id, "❌ Voice note expired. Please send it again.").await?;
                    return Ok(());
                }
            };

            bot.edit_message_text(msg.chat.id, msg.id, "⏳ Processing voice note...").await?;

            // Download audio from Telegram
            let file = bot.get_file(&file_id).await
                .map_err(|e| teloxide::RequestError::Api(teloxide::ApiError::Unknown(e.to_string())))?;
            let url = format!("https://api.telegram.org/file/bot{}/{}", bot.token(), file.path);
            let audio_bytes = match state.http.get(&url).send().await {
                Ok(r) => match r.bytes().await {
                    Ok(b) => b.to_vec(),
                    Err(e) => { bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Failed to read audio: {}", e)).await?; return Ok(()); }
                },
                Err(e) => { bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Failed to download audio: {}", e)).await?; return Ok(()); }
            };

            match action {
                "transcribe" => {
                    bot.edit_message_text(msg.chat.id, msg.id, "🎤 Transcribing...").await?;
                    match voice::transcribe(&state.http, audio_bytes, &state.config.openai_api_key, &state.config.whisper_url).await {
                        Ok(text) if text.is_empty() => {
                            bot.edit_message_text(msg.chat.id, msg.id, "😕 Could not transcribe anything. Try speaking more clearly.").await?;
                        }
                        Ok(text) => {
                            bot.edit_message_text(msg.chat.id, msg.id, format!("🔍 I heard: {}\nSearching...", text)).await?;
                            let sent = bot.send_message(msg.chat.id, format!("Results for: {}", text)).await?;
                            match deemix::search(&state, &text, "track").await {
                                Ok(results) if results.is_empty() => {
                                    bot.edit_message_text(msg.chat.id, sent.id, format!("😕 No results for: {}", text)).await?;
                                }
                                Ok(results) => {
                                    let mut buttons: Vec<Vec<InlineKeyboardButton>> = results.iter().map(|item| {
                                        let label = format!("🎵 {} — {}", item["title"].as_str().unwrap_or("?"), item["artist"]["name"].as_str().unwrap_or("?"));
                                        let label = if label.chars().count() > 60 { format!("{}...", label.chars().take(57).collect::<String>()) } else { label };
                                        vec![InlineKeyboardButton::callback(label, format!("dl:{}", item["link"].as_str().unwrap_or("")))]
                                    }).collect();
                                    buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);
                                    bot.edit_message_text(msg.chat.id, sent.id, format!("Results for '{}':", text))
                                        .reply_markup(InlineKeyboardMarkup::new(buttons)).await?;
                                }
                                Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Search failed: {}", e)).await?; }
                            }
                        }
                        Err(e) => { bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Transcription failed: {}", e)).await?; }
                    }
                }
                "recognize" => {
                    bot.edit_message_text(msg.chat.id, msg.id, "🎵 Recognizing song...").await?;
                    match voice::recognize(&state.http, audio_bytes, &state.config.audd_api_key).await {
                        Ok((title, artist)) => {
                            let query = format!("{} {}", title, artist);
                            bot.edit_message_text(msg.chat.id, msg.id, format!("🎵 Found: {} — {}\nSearching on Deezer...", title, artist)).await?;
                            let sent = bot.send_message(msg.chat.id, format!("Results for: {}", query)).await?;
                            match deemix::search(&state, &query, "track").await {
                                Ok(results) if results.is_empty() => {
                                    bot.edit_message_text(msg.chat.id, sent.id, format!("😕 No results on Deezer for: {}", query)).await?;
                                }
                                Ok(results) => {
                                    let mut buttons: Vec<Vec<InlineKeyboardButton>> = results.iter().map(|item| {
                                        let label = format!("🎵 {} — {}", item["title"].as_str().unwrap_or("?"), item["artist"]["name"].as_str().unwrap_or("?"));
                                        let label = if label.chars().count() > 60 { format!("{}...", label.chars().take(57).collect::<String>()) } else { label };
                                        vec![InlineKeyboardButton::callback(label, format!("dl:{}", item["link"].as_str().unwrap_or("")))]
                                    }).collect();
                                    buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);
                                    bot.edit_message_text(msg.chat.id, sent.id, format!("Results for {} — {}:", title, artist))
                                        .reply_markup(InlineKeyboardMarkup::new(buttons)).await?;
                                }
                                Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Search failed: {}", e)).await?; }
                            }
                        }
                        Err(e) => { bot.edit_message_text(msg.chat.id, msg.id, format!("❌ Recognition failed: {}", e)).await?; }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// ── Core Helpers ──────────────────────────────────────────────────────────────

async fn resolve_short_link(http: &Client, url: &str) -> Option<String> {
    let resp = http.head(url).send().await.ok()?;
    Some(resp.url().to_string())
}

async fn queue_url(bot: &Bot, msg: &Message, state: &Arc<BotState>, url: &str) -> ResponseResult<()> {
    let cap = match DEEZER_URL_RE.captures(url) {
        Some(c) => c,
        None => {
            bot.send_message(msg.chat.id, "❌ That doesn't look like a valid Deezer URL.").await?;
            return Ok(());
        }
    };
    let kind = cap.get(1).map(|m| m.as_str()).unwrap_or("item");
    let sent = bot.send_message(msg.chat.id, format!("⏳ Queuing {}...", kind)).await?;

    match deemix::add_to_queue(state, url).await {
        Ok(_) => { bot.edit_message_text(msg.chat.id, sent.id, format!("✅ {} added to queue!", capitalize(kind))).await?; }
        Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Failed to queue: {}", e)).await?; }
    }
    Ok(())
}

async fn do_search(bot: &Bot, msg: &Message, state: &Arc<BotState>, query: &str, search_type: &str) -> ResponseResult<()> {
    let sent = bot.send_message(msg.chat.id, format!("🔍 Searching for {}...", query)).await?;

    match deemix::search(state, query, search_type).await {
        Ok(results) if results.is_empty() => { bot.edit_message_text(msg.chat.id, sent.id, "😕 No results found.").await?; }
        Ok(results) => {
            let mut buttons: Vec<Vec<InlineKeyboardButton>> = results.iter().map(|item| {
                let label = format!("{} {} — {}",
                    if search_type == "track" { "🎵" } else { "💿" },
                    item["title"].as_str().unwrap_or("?"),
                    item["artist"]["name"].as_str().unwrap_or("?")
                );
                let label = if label.chars().count() > 60 { format!("{}...", label.chars().take(57).collect::<String>()) } else { label };
                let url = item["link"].as_str().unwrap_or("").to_string();
                vec![InlineKeyboardButton::callback(label, format!("dl:{}", url))]
            }).collect();
            buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);
            bot.edit_message_text(msg.chat.id, sent.id, format!("Results for {}:", query))
                .reply_markup(InlineKeyboardMarkup::new(buttons))
                .await?;
        }
        Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Search failed: {}", e)).await?; }
    }
    Ok(())
}

async fn handle_spotify(bot: &Bot, msg: &Message, state: &Arc<BotState>, url: &str) -> ResponseResult<()> {
    let sent = bot.send_message(msg.chat.id, "🎵 Looking up Spotify link...").await?;
    let search_type = if SPOTIFY_TRACK_RE.is_match(url) { "track" } else { "album" };

    match spotify::resolve(url).await {
        Some(meta) => {
            bot.edit_message_text(msg.chat.id, sent.id, format!("🔍 Found {}\nSearching on Deezer...", meta.label)).await?;
            match deemix::search(state, &meta.query, search_type).await {
                Ok(results) if results.is_empty() => {
                    bot.edit_message_text(msg.chat.id, sent.id, format!("😕 No results found on Deezer for: {}", meta.query)).await?;
                }
                Ok(results) => {
                    let mut buttons: Vec<Vec<InlineKeyboardButton>> = results.iter().map(|item| {
                        let label = format!("🎵 {} — {}", item["title"].as_str().unwrap_or("?"), item["artist"]["name"].as_str().unwrap_or("?"));
                        let label = if label.chars().count() > 60 { format!("{}...", label.chars().take(57).collect::<String>()) } else { label };
                        let url = item["link"].as_str().unwrap_or("").to_string();
                        vec![InlineKeyboardButton::callback(label, format!("dl:{}", url))]
                    }).collect();
                    buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);
                    bot.edit_message_text(msg.chat.id, sent.id, format!("Results for {}:", meta.query))
                        .reply_markup(InlineKeyboardMarkup::new(buttons))
                        .await?;
                }
                Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ Search failed: {}", e)).await?; }
            }
        }
        None => { bot.edit_message_text(msg.chat.id, sent.id, "❌ Could not resolve Spotify link. Try /search instead.").await?; }
    }
    Ok(())
}

async fn handle_updatearl(bot: &Bot, msg: &Message, state: &Arc<BotState>, arl: &str) -> ResponseResult<()> {
    let sent = bot.send_message(msg.chat.id, "🔄 Validating new ARL...").await?;

    match deemix::login_arl(state, arl).await {
        Ok(_username) => {
            bot.edit_message_text(msg.chat.id, sent.id, format!("✅ Logged in!\n🔄 Updating .env file...")).await?;

            match std::fs::read_to_string(&state.config.env_file) {
                Ok(contents) => {
                    let updated = regex::Regex::new(r"DEEMIX_ARL=.*").unwrap()
                        .replace(&contents, format!("DEEMIX_ARL={}", arl))
                        .to_string();
                    if let Err(e) = std::fs::write(&state.config.env_file, &updated) {
                        bot.edit_message_text(msg.chat.id, sent.id, format!("⚠️ Logged in but could not update .env: {}", e)).await?;
                        return Ok(());
                    }
                }
                Err(e) => {
                    bot.edit_message_text(msg.chat.id, sent.id, format!("⚠️ Logged in but could not read .env: {}", e)).await?;
                    return Ok(());
                }
            }

            bot.edit_message_text(msg.chat.id, sent.id, "✅ ARL updated and saved! Downloads will use the new ARL immediately.").await?;
        }
        Err(e) => { bot.edit_message_text(msg.chat.id, sent.id, format!("❌ ARL rejected by deemix: {}", e)).await?; }
    }
    Ok(())
}


async fn handle_voice_note(
    bot: &Bot,
    msg: &Message,
    state: &Arc<BotState>,
    file_id: String,
) -> ResponseResult<()> {
    let user_settings = users::get_or_create(&state.users, msg.chat.id.0);
    let whisper_on = state.config.whisper_enabled() && user_settings.voice_search;
    let audd_on = state.config.audd_enabled() && user_settings.song_recognition;

    if !whisper_on && !audd_on {
        bot.send_message(
            msg.chat.id,
            "⚠️ Voice features are not configured or disabled.
Check /settings or ask your admin to add API keys.",
        )
        .await?;
        return Ok(());
    }

    // Show options based on what is enabled
    let mut buttons = vec![];
    if whisper_on {
        buttons.push(vec![InlineKeyboardButton::callback(
            "🗣️ Search what I said",
            format!("voice_search:{}", file_id),
        )]);
    }
    if audd_on {
        buttons.push(vec![InlineKeyboardButton::callback(
            "🎵 Recognize the song",
            format!("voice_recognize:{}", file_id),
        )]);
    }
    buttons.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);

    bot.send_message(msg.chat.id, "🎤 What should I do with this voice note?")
        .reply_markup(InlineKeyboardMarkup::new(buttons))
        .await?;

    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
