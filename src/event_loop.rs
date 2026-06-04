use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::media::FfmpegRequest;
use crate::mpv_stream::MpvStream;

#[derive(Clone)]
pub struct Subtitle {
    pub id: u64,
    pub text: String,
    pub sub_start: f64,
    pub sub_end: f64,
    pub media_path: String,
    pub aid: i64,
}

#[derive(Clone)]
pub enum SubtitleEvent {
    New(Subtitle),
    MediaChanged(String),
}

struct SharedState {
    subtitles: RwLock<HashMap<u64, Subtitle>>,
    current_media_path: RwLock<Option<String>>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            subtitles: RwLock::new(HashMap::new()),
            current_media_path: RwLock::new(None),
        })
    }
}

struct PendingSubtitle {
    id: u64,
    text: String,
    responses: [Option<serde_json::Value>; 2], // sub_start, sub_end
}

impl PendingSubtitle {
    fn new(id: u64, text: String) -> Self {
        Self {
            id,
            text,
            responses: Default::default(),
        }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 2 {
            self.responses[index] = Some(value);
        }
    }

    fn is_complete(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    fn into_subtitle(self, media_path: String, aid: i64, delay: f64) -> Subtitle {
        Subtitle {
            id: self.id,
            text: self.text,
            sub_start: self.responses[0].as_ref().unwrap().as_f64().unwrap() + delay,
            sub_end: self.responses[1].as_ref().unwrap().as_f64().unwrap() + delay,
            media_path,
            aid,
        }
    }
}

async fn query_mpv_property(
    mpv: &mut MpvStream,
    property: &str,
    request_id: u64,
) -> std::io::Result<serde_json::Value> {
    let cmd = format!(
        "{{\"command\":[\"get_property\",\"{}\"],\"request_id\":{}}}\n",
        property, request_id
    );
    mpv.write_all(cmd.as_bytes()).await?;

    let mut line = String::new();
    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "mpv IPC closed while waiting for property response",
            ));
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line)
            && json.get("request_id").and_then(|v| v.as_u64()) == Some(request_id)
        {
            return Ok(json);
        }
    }
}

async fn query_mpv_property_with_timeout(
    mpv: &mut MpvStream,
    property: &str,
    request_id: u64,
) -> std::io::Result<serde_json::Value> {
    timeout(
        Duration::from_secs(1),
        query_mpv_property(mpv, property, request_id),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("Timed out querying mpv property '{}'", property),
        )
    })?
}

async fn get_mpv_pid(mpv: &mut MpvStream) -> std::io::Result<u32> {
    let json = match query_mpv_property_with_timeout(mpv, "pid", 1).await {
        Ok(json) => json,
        Err(_) => query_mpv_property_with_timeout(mpv, "process-id", 2).await?,
    };
    let status = json.get("error").and_then(|e| e.as_str()).unwrap_or("");
    if status != "success" {
        return Err(std::io::Error::other(
            format!("mpv returned error querying PID: {}", status),
        ));
    }

    let pid = json
        .get("data")
        .and_then(|d| {
            d.as_u64()
                .or_else(|| d.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mpv returned non-integer PID",
            )
        })?;

    u32::try_from(pid)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "mpv PID out of range"))
}

pub async fn run_server(
    socket_path: &str,
    port: u16,
    expected_mpv_pid: Option<u32>,
) -> std::io::Result<()> {
    let mut mpv = MpvStream::connect(socket_path).await?;
    if let Some(expected) = expected_mpv_pid {
        let actual = get_mpv_pid(&mut mpv).await?;
        if actual != expected {
            return Err(std::io::Error::other(
                format!(
                    "MPV_IPC_PID_MISMATCH expected={} actual={} socket={}",
                    expected, actual, socket_path
                ),
            ));
        }
    }
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!(
        "WebSocket server listening on {}",
        listener
            .local_addr()
            .map_or_else(|_| format!("port {}", port), |a| a.to_string())
    );

    let state = SharedState::new();
    let (subtitle_tx, _) = broadcast::channel::<SubtitleEvent>(64);

    let mpv_state = state.clone();
    let mpv_tx = subtitle_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_mpv(mpv, mpv_state, mpv_tx).await {
            error!("MPV handler error: {}", e);
        }
        info!("MPV connection closed, shutting down.");
        std::process::exit(0);
    });

    let mut client_id = 0u64;
    loop {
        let (stream, addr) = listener.accept().await?;
        client_id += 1;
        let id = client_id;

        let client_state = state.clone();
        let client_rx = subtitle_tx.subscribe();

        tokio::spawn(async move {
            info!("[client:{}] Connected from {}", id, addr);
            if let Err(e) = handle_client(stream, id, client_state, client_rx).await {
                debug!("[client:{}] Disconnected: {}", id, e);
            } else {
                debug!("[client:{}] Disconnected", id);
            }
        });
    }
}

async fn handle_mpv(
    mut mpv: MpvStream,
    state: Arc<SharedState>,
    tx: broadcast::Sender<SubtitleEvent>,
) -> std::io::Result<()> {
    mpv.write_all(
        b"{\"command\":[\"observe_property\",1,\"sub-text\"]}\n\
          {\"command\":[\"observe_property\",3,\"path\"]}\n\
          {\"command\":[\"observe_property\",4,\"aid\"]}\n\
          {\"command\":[\"observe_property\",5,\"sub-delay\"]}\n",
    )
    .await?;
    info!("Connected to mpv, observing subtitle changes");

    let mut current_path: Option<String> = None;
    // Latest selected audio track id, kept current via the `aid` observe (id 4)
    // instead of being queried per subtitle. Defaults to track 1 until mpv sends
    // the initial property-change for the observe.
    let mut current_aid: i64 = 1;
    // Latest subtitle delay, kept current via the sub-delay observe (id 5) and
    // applied to subtitle timing at emit time.
    let mut current_sub_delay: f64 = 0.0;
    let mut pending: HashMap<u64, PendingSubtitle> = HashMap::new();
    let mut next_subtitle_id = 1u64;
    let mut next_request_id = 10u64;
    let mut line = String::new();

    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Ok(()); // EOF
        }

        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        // Handle property responses (request_id encodes: base_id + property_index)
        if let Some(request_id) = json.get("request_id").and_then(|r| r.as_u64()) {
            let base_id = request_id / 10 * 10; // Round down to base
            let prop_idx = (request_id % 10) as usize;

            if let Some(p) = pending.get_mut(&base_id)
                && let Some(data) = json.get("data").cloned()
            {
                p.set_response(prop_idx, data);
            }

            // Try to complete pending subtitles
            let completed: Vec<_> = pending
                .iter()
                .filter(|(_, p)| p.is_complete())
                .map(|(id, _)| *id)
                .collect();

            for base_id in completed {
                let media_path = current_path.clone().unwrap_or_default();
                let sub = pending
                    .remove(&base_id)
                    .unwrap()
                    .into_subtitle(media_path, current_aid, current_sub_delay);
                debug!("[sub:{}] Broadcasting", sub.id);
                state.subtitles.write().await.insert(sub.id, sub.clone());
                let _ = tx.send(SubtitleEvent::New(sub));
            }
            continue;
        }

        if json.get("event") != Some(&serde_json::json!("property-change")) {
            continue;
        }

        // Media file path changed (observer id 3)
        if json.get("id").and_then(|v| v.as_u64()) == Some(3) {
            if let Some(path) = json.get("data").and_then(|d| d.as_str()) {
                let path = path.to_string();
                if current_path.as_deref() != Some(&path) {
                    current_path = Some(path.clone());
                    *state.current_media_path.write().await = Some(path.clone());
                    let _ = tx.send(SubtitleEvent::MediaChanged(path));
                }
            }
            continue;
        }

        // Audio track changed (observer id 4)
        if json.get("id").and_then(|v| v.as_u64()) == Some(4) {
            if let Some(aid) = json.get("data").and_then(|d| d.as_i64()) {
                current_aid = aid;
            }
            continue;
        }

        // Subtitle delay changed (observer id 5)
        if json.get("id").and_then(|v| v.as_u64()) == Some(5) {
            if let Some(delay) = json.get("data").and_then(|d| d.as_f64()) {
                current_sub_delay = delay;
            }
            continue;
        }

        // Handle subtitle property changes (observer id 1)
        if let Some(text) = json
            .get("data")
            .and_then(|d| d.as_str())
            .filter(|s| !s.is_empty())
        {
            let subtitle_id = next_subtitle_id;
            next_subtitle_id += 1;

            let base_id = next_request_id;
            next_request_id += 10;

            // Query the per-subtitle timing properties; aid is observed
            // separately (id 4) and read from `current_aid`.
            let cmd = format!(
                concat!(
                    "{{\"command\":[\"get_property\",\"sub-start\"],\"request_id\":{0}}}\n",
                    "{{\"command\":[\"get_property\",\"sub-end\"],\"request_id\":{1}}}\n"
                ),
                base_id,
                base_id + 1
            );

            mpv.write_all(cmd.as_bytes()).await?;
            pending.insert(base_id, PendingSubtitle::new(subtitle_id, text.to_string()));
            info!("[sub:{}] {}", subtitle_id, text);
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    id: u64,
    state: Arc<SharedState>,
    mut subtitle_rx: broadcast::Receiver<SubtitleEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = accept_async(stream).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    if let Some(path) = state.current_media_path.read().await.clone() {
        let msg = serde_json::json!({ "type": "media_changed", "path": path });
        ws_tx.send(Message::Text(msg.to_string().into())).await?;
    }

    loop {
        tokio::select! {
            Ok(event) = subtitle_rx.recv() => {
                let msg = match event {
                    SubtitleEvent::New(sub) => serde_json::json!({
                        "type": "subtitle",
                        "id": sub.id,
                        "subtitle": sub.text,
                        "sub_start": sub.sub_start,
                        "sub_end": sub.sub_end,
                    }),
                    SubtitleEvent::MediaChanged(path) => serde_json::json!({
                        "type": "media_changed",
                        "path": path,
                    }),
                };
                ws_tx.send(Message::Text(msg.to_string().into())).await?;
            }

            Some(msg) = ws_rx.next() => {
                let msg = msg?;
                if let Message::Text(text) = msg {
                    if let Some(response) = handle_request(&text, id, &state).await {
                        ws_tx.send(Message::Text(response.into())).await?;
                    }
                } else if msg.is_close() {
                    return Ok(());
                }
            }

            else => return Ok(()),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum ProtocolRequest {
    Thumbnail {
        id: u64,
        end_id: Option<u64>,
        image_config: Option<crate::media::ImageConfig>,
    },
    Audio {
        id: u64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        audio_config: Option<crate::media::AudioConfig>,
    },
    AudioRange {
        start_id: u64,
        end_id: u64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        audio_config: Option<crate::media::AudioConfig>,
    },
}

async fn handle_request(text: &str, client_id: u64, state: &Arc<SharedState>) -> Option<String> {
    let request: ProtocolRequest = serde_json::from_str(text).ok()?;

    match request {
        ProtocolRequest::AudioRange {
            start_id,
            end_id,
            offset_start,
            offset_end,
            audio_config,
        } => {
            let store = state.subtitles.read().await;
            let start = store.get(&start_id)?;
            let end = store.get(&end_id)?;
            let ffmpeg_req = FfmpegRequest::audio_range(
                start.sub_start,
                end.sub_end,
                &start.media_path,
                start.aid,
                offset_start,
                offset_end,
                audio_config,
            );
            drop(store);

            info!(
                "[client:{}] Requesting audio_range from subtitle {} to {}",
                client_id, start_id, end_id
            );

            let data = tokio::task::spawn_blocking(move || ffmpeg_req.execute())
                .await
                .ok()?;

            Some(
                serde_json::json!({
                    "type": "audio_range",
                    "start_id": start_id,
                    "end_id": end_id,
                    "data": data,
                })
                .to_string(),
            )
        }
        _ => {
            let (subtitle_id, media_type, ffmpeg_req) = match request {
                ProtocolRequest::Thumbnail { id, end_id, image_config } => {
                    let store = state.subtitles.read().await;
                    let mut sub = store.get(&id)?.clone();
                    if let Some(eid) = end_id
                        && let Some(end_sub) = store.get(&eid) {
                            sub.sub_end = end_sub.sub_end;
                        }
                    drop(store);
                    (
                        id,
                        "thumbnail",
                        FfmpegRequest::thumbnail(&sub, image_config),
                    )
                }
                ProtocolRequest::Audio {
                    id,
                    offset_start,
                    offset_end,
                    audio_config,
                } => {
                    let store = state.subtitles.read().await;
                    let sub = store.get(&id)?.clone();
                    drop(store);
                    (
                        id,
                        "audio",
                        FfmpegRequest::audio(&sub, offset_start, offset_end, audio_config),
                    )
                }
                _ => unreachable!(),
            };

            info!(
                "[client:{}] Requesting {} for subtitle {}",
                client_id, media_type, subtitle_id
            );

            let req_type = media_type.to_string();
            let data = tokio::task::spawn_blocking(move || ffmpeg_req.execute())
                .await
                .ok()?;

            if data.is_some() {
                debug!("[media] {} ready for subtitle {}", req_type, subtitle_id);
            } else {
                warn!(
                    "[media] Failed to generate {} for subtitle {}",
                    req_type, subtitle_id
                );
            }

            Some(
                serde_json::json!({
                    "type": req_type,
                    "id": subtitle_id,
                    "data": data,
                })
                .to_string(),
            )
        }
    }
}
