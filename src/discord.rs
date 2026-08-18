use amp_api::{PlaybackExtension, PlaybackInfo};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct DiscordRPC {
    client: Arc<Mutex<Option<DiscordIpcClient>>>,
    last_title: Mutex<String>,
    last_paused: Mutex<bool>,
    start_time: Mutex<i64>,
    has_activity: Mutex<bool>,
}

impl DiscordRPC {
    pub fn new() -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let client = Arc::new(Mutex::new(None));
        let client_clone = client.clone();

        tokio::spawn(async move {
            let client_id = "622783718783844356";
            loop {
                let (tx, rx) = tokio::sync::oneshot::channel();
                std::thread::spawn(move || {
                    let res = (|| {
                        let mut client = DiscordIpcClient::new(client_id);
                        client.connect().ok()?;
                        Some(client)
                    })();
                    let _ = tx.send(res);
                });

                match tokio::time::timeout(Duration::from_secs(4), rx).await {
                    Ok(Ok(Some(ipc_client))) => {
                        eprintln!("[DiscordRPC] Connected to Discord client successfully");
                        let mut client_lock = client_clone.lock().unwrap();
                        *client_lock = Some(ipc_client);
                        break;
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });

        Self {
            client,
            last_title: Mutex::new(String::new()),
            last_paused: Mutex::new(false),
            start_time: Mutex::new(start_time),
            has_activity: Mutex::new(false),
        }
    }
}

impl Default for DiscordRPC {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackExtension for DiscordRPC {
    fn on_playback_update(&self, info: PlaybackInfo) {
        if info.title.is_empty() {
            return;
        }

        let mut client_lock = self.client.lock().unwrap();
        if let Some(client) = &mut *client_lock {
            let mut last_title = self.last_title.lock().unwrap();
            let mut last_paused = self.last_paused.lock().unwrap();
            let mut has_activity = self.has_activity.lock().unwrap();
            let mut start_time = self.start_time.lock().unwrap();

            if *last_title == info.title && *last_paused == info.is_paused && *has_activity {
                return;
            }

            if *last_title != info.title {
                *start_time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
            }

            *last_title = info.title.clone();
            *last_paused = info.is_paused;
            *has_activity = true;

            let mut act = activity::Activity::new();

            let state_str = if info.is_paused {
                "Paused".to_string()
            } else if !info.artist.is_empty() {
                info.artist
            } else {
                "Watching".to_string()
            };

            act = act
                .details(&info.title)
                .state(&state_str)
                .activity_type(activity::ActivityType::Watching);

            if !info.is_paused && info.duration_secs > 0 {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                let start = now - info.position_secs;
                let end = start + info.duration_secs;
                act = act.timestamps(activity::Timestamps::new().start(start).end(end));
            } else if !info.is_paused {
                act = act.timestamps(activity::Timestamps::new().start(*start_time));
            }

            if let Err(e) = client.set_activity(act) {
                eprintln!("[DiscordRPC] Failed to update activity: {}", e);
                let _ = client.reconnect();
            }
        }
    }

    fn on_playback_stop(&self) {
        let mut has_activity = self.has_activity.lock().unwrap();
        if *has_activity {
            if let Some(client) = &mut *self.client.lock().unwrap() {
                let _ = client.clear_activity();
            }
            *has_activity = false;
            self.last_title.lock().unwrap().clear();
        }
    }

    fn set_controller(&self, _controller: Arc<dyn amp_api::PlaybackController>) {}
}
