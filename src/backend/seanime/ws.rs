#![allow(dead_code)]
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWsMessage {
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum SeanimeWsEvent {
    LibraryUpdated,
    ProgressUpdated {
        media_id: i32,
        episode_number: i32,
    },
    ScanProgress {
        message: String,
    },
    PlaybackState {
        is_playing: bool,
        position_secs: i64,
    },
    RawEvent {
        event_type: String,
        payload: serde_json::Value,
    },
}

#[derive(Clone)]
pub struct SeanimeWsSender {
    tx: mpsc::UnboundedSender<String>,
}

impl SeanimeWsSender {
    pub fn send_video_loaded(
        &self,
        playback_id: &str,
        media_id: i32,
        episode_number: i32,
        _duration: f64,
    ) {
        let payload = serde_json::json!({
            "type": "videocore",
            "payload": {
                "clientId": "amp-desktop",
                "type": "video-loaded",
                "payload": {
                    "state": {
                        "clientId": "amp-desktop",
                        "playerType": "native",
                        "playbackInfo": {
                            "id": playback_id,
                            "streamType": "localfile",
                            "media": {
                                "id": media_id
                            },
                            "episode": {
                                "episodeNumber": episode_number
                            }
                        }
                    }
                }
            }
        });
        let _ = self.tx.send(payload.to_string());
    }

    pub fn send_video_status(
        &self,
        playback_id: &str,
        current_time: f64,
        duration: f64,
        is_paused: bool,
    ) {
        let payload = serde_json::json!({
            "type": "videocore",
            "payload": {
                "clientId": "amp-desktop",
                "type": "video-status",
                "payload": {
                    "id": playback_id,
                    "clientId": "amp-desktop",
                    "currentTime": current_time,
                    "duration": duration,
                    "paused": is_paused
                }
            }
        });
        let _ = self.tx.send(payload.to_string());
    }

    pub fn send_video_completed(&self, current_time: f64, duration: f64) {
        let payload = serde_json::json!({
            "type": "videocore",
            "payload": {
                "clientId": "amp-desktop",
                "type": "video-completed",
                "payload": {
                    "currentTime": current_time,
                    "duration": duration,
                    "paused": true
                }
            }
        });
        let _ = self.tx.send(payload.to_string());
    }

    pub fn send_video_terminated(&self, playback_id: &str) {
        let payload = serde_json::json!({
            "type": "videocore",
            "payload": {
                "clientId": "amp-desktop",
                "type": "video-terminated",
                "payload": {
                    "id": playback_id,
                    "clientId": "amp-desktop",
                    "playerType": "native",
                    "playbackType": "localfile"
                }
            }
        });
        let _ = self.tx.send(payload.to_string());
    }
}

pub struct SeanimeWsListener {
    running: Arc<AtomicBool>,
    outbound_tx: mpsc::UnboundedSender<String>,
    outbound_rx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<String>>>>,
}

impl SeanimeWsListener {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            running: Arc::new(AtomicBool::new(false)),
            outbound_tx: tx,
            outbound_rx: Arc::new(tokio::sync::Mutex::new(Some(rx))),
        }
    }

    pub fn sender(&self) -> SeanimeWsSender {
        SeanimeWsSender {
            tx: self.outbound_tx.clone(),
        }
    }

    pub fn start(
        &self,
        base_http_url: String,
        event_sender: mpsc::UnboundedSender<SeanimeWsEvent>,
    ) -> Arc<AtomicBool> {
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);
        let running_clone = running.clone();
        let outbound_rx_lock = self.outbound_rx.clone();

        tokio::spawn(async move {
            let mut outbound_receiver = {
                let mut guard = outbound_rx_lock.lock().await;
                guard.take()
            };

            let ws_url_str = if base_http_url.starts_with("https://") {
                format!("{}/events", base_http_url.replacen("https://", "wss://", 1))
            } else if base_http_url.starts_with("http://") {
                format!("{}/events", base_http_url.replacen("http://", "ws://", 1))
            } else {
                format!("ws://{}/events", base_http_url)
            };

            let mut backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(10);

            while running_clone.load(Ordering::SeqCst) {
                eprintln!("[SeanimeWS] Connecting to {}...", ws_url_str);
                match connect_async(&ws_url_str).await {
                    Ok((ws_stream, response)) => {
                        eprintln!(
                            "[SeanimeWS] Connected successfully! Status: {}",
                            response.status()
                        );
                        backoff = Duration::from_millis(500);

                        let (mut write, mut read) = ws_stream.split();

                        let running_inner = running_clone.clone();
                        let event_tx = event_sender.clone();

                        loop {
                            if !running_inner.load(Ordering::SeqCst) {
                                break;
                            }

                            tokio::select! {
                                msg_res = read.next() => {
                                    match msg_res {
                                        Some(Ok(msg)) => {
                                            if msg.is_text() {
                                                if let Ok(text) = msg.to_text()
                                                    && let Ok(raw) = serde_json::from_str::<RawWsMessage>(text) {
                                                        let event_type = raw.event_type.unwrap_or_default();
                                                        let payload = raw.payload.unwrap_or(serde_json::Value::Null);

                                                        let parsed_event = match event_type.as_str() {
                                                            t if t.contains("Library") || t.contains("Collection") => {
                                                                SeanimeWsEvent::LibraryUpdated
                                                            }
                                                            t if t.contains("Progress") => {
                                                                let media_id = payload["mediaId"].as_i64().unwrap_or(0) as i32;
                                                                let episode_number = payload["episodeNumber"].as_i64().unwrap_or(0) as i32;
                                                                SeanimeWsEvent::ProgressUpdated {
                                                                    media_id,
                                                                    episode_number,
                                                                }
                                                            }
                                                            t if t.contains("Scan") => {
                                                                let message = payload["message"].as_str().unwrap_or_default().to_string();
                                                                SeanimeWsEvent::ScanProgress { message }
                                                            }
                                                            _ => SeanimeWsEvent::RawEvent {
                                                                event_type,
                                                                payload,
                                                            },
                                                        };

                                                        let _ = event_tx.send(parsed_event);
                                                    }
                                            } else if msg.is_close() {
                                                eprintln!("[SeanimeWS] Server sent close frame");
                                                break;
                                            }
                                        }
                                        Some(Err(e)) => {
                                            eprintln!("[SeanimeWS] Error reading: {:?}", e);
                                            break;
                                        }
                                        None => break,
                                    }
                                }
                                outbound_opt = async {
                                    if let Some(ref mut rx) = outbound_receiver {
                                        rx.recv().await
                                    } else {
                                        futures::future::pending().await
                                    }
                                } => {
                                    if let Some(outbound_msg) = outbound_opt
                                        && let Err(e) = write.send(Message::Text(outbound_msg.into())).await {
                                            eprintln!("[SeanimeWS] Error sending message: {:?}", e);
                                            break;
                                        }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[SeanimeWS] Connection failed: {:?}. Retrying in {:?}",
                            e, backoff
                        );
                    }
                }

                if running_clone.load(Ordering::SeqCst) {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }

            eprintln!("[SeanimeWS] Listener stopped");
        });

        running
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for SeanimeWsListener {
    fn default() -> Self {
        Self::new()
    }
}
