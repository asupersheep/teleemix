use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub restart_notifications: bool,
    #[serde(default = "default_true")]
    pub voice_search: bool,
    #[serde(default = "default_true")]
    pub song_recognition: bool,
}

fn default_true() -> bool { true }

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            restart_notifications: false, // off by default
            voice_search: true,
            song_recognition: true,
        }
    }
}

pub type UsersDb = Arc<RwLock<HashMap<String, UserSettings>>>;

pub fn load(path: &str) -> UsersDb {
    let map = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Arc::new(RwLock::new(map))
}

pub fn save(db: &UsersDb, path: &str) {
    if let Ok(map) = db.read() {
        if let Ok(json) = serde_json::to_string_pretty(&*map) {
            let _ = std::fs::write(path, json);
        }
    }
}

pub fn get_or_create(db: &UsersDb, user_id: i64) -> UserSettings {
    let key = user_id.to_string();
    if let Ok(map) = db.read() {
        if let Some(settings) = map.get(&key) {
            return settings.clone();
        }
    }
    // Auto-create with defaults
    let settings = UserSettings::default();
    if let Ok(mut map) = db.write() {
        map.insert(key, settings.clone());
    }
    settings
}

pub fn update<F>(db: &UsersDb, path: &str, user_id: i64, f: F)
where
    F: FnOnce(&mut UserSettings),
{
    let key = user_id.to_string();
    if let Ok(mut map) = db.write() {
        let settings = map.entry(key).or_default();
        f(settings);
    }
    save(db, path);
}

pub fn all_with_notifications(db: &UsersDb) -> Vec<i64> {
    if let Ok(map) = db.read() {
        map.iter()
            .filter(|(_, s)| s.restart_notifications)
            .filter_map(|(k, _)| k.parse::<i64>().ok())
            .collect()
    } else {
        vec![]
    }
}
