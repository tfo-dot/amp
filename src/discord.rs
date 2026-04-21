use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use std::sync::{Arc, Mutex};
use amp_api::{PlaybackExtension, AmpPlugin, PluginCapability, PlaybackInfo};

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
                eprintln!("[Discord] Attempting to connect in background...");
                
                let (tx, rx) = tokio::sync::oneshot::channel();
                std::thread::spawn(move || {
                    let res = (|| {
                        let mut client = DiscordIpcClient::new(client_id);
                        client.connect().ok()?;
                        Some(client)
                    })();
                    let _ = tx.send(res);
                });

                match tokio::time::timeout(Duration::from_secs(5), rx).await {
                    Ok(Ok(Some(client))) => {
                        eprintln!("[Discord] Connection established");
                        let mut client_lock = client_clone.lock().unwrap();
                        *client_lock = Some(client);
                        break;
                    }
                    _ => {
                        eprintln!("[Discord] Connection attempt failed or timed out");
                    }
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
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

impl PlaybackExtension for DiscordRPC {
    fn on_playback_update(&self, info: PlaybackInfo) {
        if info.title.is_empty() {
            return;
        }
        
        let mut client_lock = self.client.lock().unwrap();
        if let Some(ref mut client) = *client_lock {
            let mut last_title = self.last_title.lock().unwrap();
            let mut last_paused = self.last_paused.lock().unwrap();
            let mut has_activity = self.has_activity.lock().unwrap();
            let mut start_time = self.start_time.lock().unwrap();

            if *last_title == info.title && *last_paused == info.is_paused && *has_activity {
                return;
            }
            
            eprintln!("[Discord] Updating activity: {} - {} (paused: {})", info.title, info.artist, info.is_paused);
            
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
            
            let state = if info.is_paused {
                "Paused".to_string()
            } else {
                if !info.artist.is_empty() {
                    info.artist
                } else {
                    "Playing".to_string()
                }
            };
            
            act = act.details(&info.title).state(&state).activity_type(activity::ActivityType::Watching);

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
                eprintln!("[Discord] Failed to set activity: {}", e);
                if let Err(re) = client.reconnect() {
                    eprintln!("[Discord] Reconnect failed: {}", re);
                    *client_lock = None;
                }
            }
        }
    }

    fn on_playback_stop(&self) {
        let mut has_activity = self.has_activity.lock().unwrap();
        if *has_activity {
            if let Some(ref mut client) = *self.client.lock().unwrap() {
                let _ = client.clear_activity();
            }
            *has_activity = false;
            self.last_title.lock().unwrap().clear();
        }
    }
}

pub struct DiscordExtensionFactory;

impl AmpPlugin for DiscordExtensionFactory {
    fn id(&self) -> &'static str { "discord" }
    fn display_name(&self) -> &'static str { "Discord Rich Presence" }
    fn capabilities(&self) -> Vec<PluginCapability> {
        vec![PluginCapability::PlaybackExtension]
    }
    fn create_extension(&self) -> Result<Arc<dyn PlaybackExtension>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::new(DiscordRPC::new()))
    }
}
