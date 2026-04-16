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

/// How many unmatched secondaries to hold before dropping the oldest.
const MAX_UNMATCHED_SECONDARY: usize = 3;

/// If an unmatched secondary starts within this many seconds of the end of the
/// last successfully matched secondary, append it to that card instead of
/// dropping it.  Handles translation tracks whose lines don't align 1-to-1 with
/// the primary but continue the same sentence.
const SECONDARY_MERGE_GAP: f64 = 2.0;


#[derive(Clone)]
pub struct Subtitle {
    pub id: u64,
    pub text: String,
    pub secondary_text: Option<String>,
    pub sub_start: f64,
    pub sub_end: f64,
    pub media_path: String,
    pub aid: i64,
}

#[derive(Clone)]
pub enum SubtitleEvent {
    New(Subtitle),
    SecondaryUpdate { id: u64, text: String },
    SecondaryAppend { id: u64, text: String },
}

struct SharedState {
    subtitles: RwLock<HashMap<u64, Subtitle>>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            subtitles: RwLock::new(HashMap::new()),
        })
    }
}

// ── Pending primary subtitle ──────────────────────────────────────────────────

struct PendingPrimary {
    id: u64,
    text: String,
    // indices: 0=sub_start, 1=sub_end, 2=path, 3=aid, 4=sub_delay
    responses: [Option<serde_json::Value>; 5],
}

impl PendingPrimary {
    fn new(id: u64, text: String) -> Self {
        Self { id, text, responses: Default::default() }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 5 {
            self.responses[index] = Some(value);
        }
    }

    fn is_ready(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    fn sub_delay(&self) -> f64 {
        self.responses[4].as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0)
    }

    fn sub_start(&self) -> Option<f64> {
        Some(self.responses[0].as_ref()?.as_f64()? + self.sub_delay())
    }

    fn sub_end(&self) -> Option<f64> {
        Some(self.responses[1].as_ref()?.as_f64()? + self.sub_delay())
    }

    fn into_subtitle(self, secondary_text: Option<String>) -> Subtitle {
        let delay = self.sub_delay();
        Subtitle {
            id: self.id,
            text: self.text,
            secondary_text,
            sub_start: self.responses[0].as_ref().unwrap().as_f64().unwrap() + delay,
            sub_end:   self.responses[1].as_ref().unwrap().as_f64().unwrap() + delay,
            media_path: self.responses[2].as_ref().unwrap().as_str().unwrap().to_string(),
            aid: self.responses[3].as_ref().unwrap().as_i64().unwrap(),
        }
    }
}

// ── Pending secondary subtitle ────────────────────────────────────────────────

struct PendingSecondary {
    text: String,
    // indices: 0=secondary-sub-start, 1=secondary-sub-end, 2=secondary-sub-delay
    responses: [Option<serde_json::Value>; 3],
}

impl PendingSecondary {
    fn new(text: String) -> Self {
        Self { text, responses: Default::default() }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 3 {
            self.responses[index] = Some(value);
        }
    }

    fn is_ready(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    fn sub_delay(&self) -> f64 {
        self.responses[2].as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0)
    }

    fn sub_start(&self) -> Option<f64> {
        Some(self.responses[0].as_ref()?.as_f64()? + self.sub_delay())
    }

    fn sub_end(&self) -> Option<f64> {
        Some(self.responses[1].as_ref()?.as_f64()? + self.sub_delay())
    }
}

// ── Matching helper ───────────────────────────────────────────────────────────

/// Returns true if [p_start, p_end] and [s_start, s_end] overlap, allowing
/// `threshold` seconds of tolerance on either side.
fn ranges_overlap(p_start: f64, p_end: f64, s_start: f64, s_end: f64, threshold: f64) -> bool {
    p_start - threshold <= s_end && s_start <= p_end + threshold
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
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            if json.get("request_id").and_then(|v| v.as_u64()) == Some(request_id) {
                return Ok(json);
            }
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
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
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
    secondary_match_threshold: f64,
) -> std::io::Result<()> {
    let mut mpv = MpvStream::connect(socket_path).await?;
    if let Some(expected) = expected_mpv_pid {
        let actual = get_mpv_pid(&mut mpv).await?;
        if actual != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
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
        if let Err(e) = handle_mpv(mpv, mpv_state, mpv_tx, secondary_match_threshold).await {
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

/// Adds a secondary to the wait buffer. Returns the evicted entry (base_id, start, end)
/// if the buffer was full, so the caller can decide whether to append or drop it.
fn push_ready_secondary(
    ready_secondary: &mut Vec<(u64, f64, f64)>,
    base_id: u64,
    s_start: f64,
    s_end: f64,
) -> Option<(u64, f64, f64)> {
    ready_secondary.push((base_id, s_start, s_end));
    if ready_secondary.len() > MAX_UNMATCHED_SECONDARY {
        Some(ready_secondary.remove(0))
    } else {
        None
    }
}

async fn handle_mpv(
    mut mpv: MpvStream,
    state: Arc<SharedState>,
    tx: broadcast::Sender<SubtitleEvent>,
    secondary_match_threshold: f64,
) -> std::io::Result<()> {
    mpv.write_all(
        b"{\"command\":[\"observe_property\",1,\"sub-text\"]}\n\
          {\"command\":[\"observe_property\",2,\"secondary-sub-text\"]}\n",
    )
    .await?;
    info!("Connected to mpv, observing subtitle changes");

    // Pending primaries/secondaries keyed by base request_id.
    let mut pending_primary: HashMap<u64, PendingPrimary> = HashMap::new();
    let mut pending_secondary: HashMap<u64, PendingSecondary> = HashMap::new();

    // Secondaries that are ready but found no match at arrival time.
    // Held briefly so a primary that resolves slightly later can still claim them.
    // Vec<(base_id, sub_start, sub_end)>
    let mut ready_secondary: Vec<(u64, f64, f64)> = Vec::new();

    // The most recently emitted primary: (subtitle_id, sub_start, sub_end).
    // A secondary that resolves slightly after its primary will find it here.
    let mut last_primary: Option<(u64, f64, f64)> = None;

    // Last primary whose secondary was successfully matched: (prim_id, s_start, s_end).
    // Used to merge close-following secondaries that don't overlap any primary.
    let mut last_matched_secondary: Option<(u64, f64, f64)> = None;

    let mut next_subtitle_id = 1u64;
    let mut next_request_id = 10u64;
    let mut line = String::new();

    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Ok(());
        }

        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        // ── Property query responses ──────────────────────────────────────────
        if let Some(request_id) = json.get("request_id").and_then(|r| r.as_u64()) {
            let base_id = request_id / 10 * 10;
            let prop_idx = (request_id % 10) as usize;
            let data = json.get("data").cloned();

            if let Some(p) = pending_primary.get_mut(&base_id) {
                match data {
                    Some(d) => p.set_response(prop_idx, d),
                    None => {
                        // IPC error — sentinel so is_ready() can fire and we can discard.
                        let fallback: serde_json::Value = match prop_idx {
                            0 | 1 => f64::MAX.into(),
                            2 => "".into(),
                            3 => (-1i64).into(),
                            _ => 0.0f64.into(),
                        };
                        p.set_response(prop_idx, fallback);
                    }
                }
                if p.is_ready() {
                    let sub_start = pending_primary[&base_id].sub_start().unwrap_or(f64::MAX);
                    let sub_end = pending_primary[&base_id].sub_end().unwrap_or(sub_start);
                    let primary = pending_primary.remove(&base_id).unwrap();
                    let subtitle_id = primary.id;

                    // Discard if timing is invalid (IPC error on sub-start/end).
                    if sub_start >= f64::MAX / 2.0 {
                        continue;
                    }

                    // Emit immediately — secondary arrives via SecondaryUpdate.
                    emit(primary, None, &state, &tx).await;

                    // Check if a secondary was already waiting for this primary.
                    let sec_pos = ready_secondary.iter().position(|(_, ss, se)| {
                        ranges_overlap(sub_start, sub_end, *ss, *se, secondary_match_threshold)
                    });
                    if let Some(pos) = sec_pos {
                        let (s_base, ss, se) = ready_secondary.remove(pos);
                        if let Some(sec) = pending_secondary.remove(&s_base) {
                            if !sec.text.is_empty() {
                                let _ = tx.send(SubtitleEvent::SecondaryUpdate { id: subtitle_id, text: sec.text });
                                last_matched_secondary = Some((subtitle_id, ss, se));
                                // Primary is matched; don't expose it for a second secondary.
                                last_primary = None;
                                continue;
                            }
                        }
                    }

                    // No secondary yet — remember this primary for a late-arriving secondary.
                    last_primary = Some((subtitle_id, sub_start, sub_end));
                }
            } else if let Some(s) = pending_secondary.get_mut(&base_id) {
                if let Some(d) = data {
                    s.set_response(prop_idx, d);
                } else {
                    // secondary-sub-start/end errors when no secondary line is active.
                    // Use sentinel for timing indices; default delay to 0.
                    let fallback = if prop_idx < 2 { f64::MAX } else { 0.0 };
                    s.set_response(prop_idx, serde_json::Value::from(fallback));
                }
                if s.is_ready() {
                    let s_start = pending_secondary[&base_id].sub_start().unwrap_or(f64::MAX);

                    // Sentinel means no active secondary line — discard immediately.
                    if s_start == f64::MAX {
                        pending_secondary.remove(&base_id);
                        continue;
                    }

                    let s_end = pending_secondary[&base_id].sub_end().unwrap_or(s_start);

                    // Try to match against the last emitted primary.
                    let matched_primary = last_primary.filter(|(_, p_start, p_end)| {
                        ranges_overlap(*p_start, *p_end, s_start, s_end, secondary_match_threshold)
                    });

                    if let Some((prim_id, _, _)) = matched_primary {
                        last_primary = None;
                        let sec = pending_secondary.remove(&base_id).unwrap();
                        if !sec.text.is_empty() {
                            debug!("[sub:{}] Secondary match", prim_id);
                            let _ = tx.send(SubtitleEvent::SecondaryUpdate { id: prim_id, text: sec.text });
                            last_matched_secondary = Some((prim_id, s_start, s_end));
                        }
                    } else {
                        // Buffer and wait for the next primary to claim it.
                        // Only append to a previous card if the buffer is full and it's about to be dropped.
                        if let Some((evicted_base, ev_start, ev_end)) =
                            push_ready_secondary(&mut ready_secondary, base_id, s_start, s_end)
                        {
                            let appended = if let Some((prev_id, _, prev_end)) = last_matched_secondary {
                                if ev_start - prev_end <= SECONDARY_MERGE_GAP {
                                    if let Some(sec) = pending_secondary.remove(&evicted_base) {
                                        if !sec.text.is_empty() {
                                            debug!("[sub:{}] Appending evicted secondary (gap {:.2}s)", prev_id, ev_start - prev_end);
                                            let _ = tx.send(SubtitleEvent::SecondaryAppend { id: prev_id, text: sec.text });
                                            last_matched_secondary = Some((prev_id, ev_start, ev_end));
                                            true
                                        } else { false }
                                    } else { false }
                                } else { false }
                            } else { false };

                            if !appended {
                                pending_secondary.remove(&evicted_base);
                                debug!("Dropped unmatched secondary subtitle");
                            }
                        }
                    }
                }
            }

            continue;
        }

        // ── Property-change events ────────────────────────────────────────────
        if json.get("event") != Some(&serde_json::json!("property-change")) {
            continue;
        }

        let observer_id = json.get("id").and_then(|v| v.as_u64());

        // ── Secondary subtitle text changed ───────────────────────────────────
        if observer_id == Some(2) {
            let text = match json
                .get("data")
                .and_then(|d| d.as_str())
                .filter(|s| !s.is_empty())
            {
                Some(t) => t.to_string(),
                None => continue,
            };

            let base_id = next_request_id;
            next_request_id += 10;

            let cmd = format!(
                concat!(
                    "{{\"command\":[\"get_property\",\"secondary-sub-start\"],\"request_id\":{0}}}\n",
                    "{{\"command\":[\"get_property\",\"secondary-sub-end\"],\"request_id\":{1}}}\n",
                    "{{\"command\":[\"get_property\",\"secondary-sub-delay\"],\"request_id\":{2}}}\n"
                ),
                base_id,
                base_id + 1,
                base_id + 2
            );
            mpv.write_all(cmd.as_bytes()).await?;
            pending_secondary.insert(base_id, PendingSecondary::new(text));
            continue;
        }

        // ── Primary subtitle text changed ─────────────────────────────────────
        if observer_id == Some(1) {
            if let Some(text) = json
                .get("data")
                .and_then(|d| d.as_str())
                .filter(|s| !s.is_empty())
            {
                let subtitle_id = next_subtitle_id;
                next_subtitle_id += 1;
                let base_id = next_request_id;
                next_request_id += 10;

                let cmd = format!(
                    concat!(
                        "{{\"command\":[\"get_property\",\"sub-start\"],\"request_id\":{0}}}\n",
                        "{{\"command\":[\"get_property\",\"sub-end\"],\"request_id\":{1}}}\n",
                        "{{\"command\":[\"get_property\",\"path\"],\"request_id\":{2}}}\n",
                        "{{\"command\":[\"get_property\",\"aid\"],\"request_id\":{3}}}\n",
                        "{{\"command\":[\"get_property\",\"sub-delay\"],\"request_id\":{4}}}\n"
                    ),
                    base_id,
                    base_id + 1,
                    base_id + 2,
                    base_id + 3,
                    base_id + 4
                );

                mpv.write_all(cmd.as_bytes()).await?;
                pending_primary.insert(base_id, PendingPrimary::new(subtitle_id, text.to_string()));
                info!("[sub:{}] {}", subtitle_id, text);
            }
        }
    }
}

async fn emit(
    primary: PendingPrimary,
    secondary_text: Option<String>,
    state: &Arc<SharedState>,
    tx: &broadcast::Sender<SubtitleEvent>,
) {
    let sub = primary.into_subtitle(secondary_text);
    debug!("[sub:{}] Broadcasting (secondary: {:?})", sub.id, sub.secondary_text.is_some());
    state.subtitles.write().await.insert(sub.id, sub.clone());
    let _ = tx.send(SubtitleEvent::New(sub));
}

async fn handle_client(
    stream: TcpStream,
    id: u64,
    state: Arc<SharedState>,
    mut subtitle_rx: broadcast::Receiver<SubtitleEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = accept_async(stream).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    loop {
        tokio::select! {
            Ok(event) = subtitle_rx.recv() => {
                let msg = match event {
                    SubtitleEvent::New(sub) => serde_json::json!({
                        "type": "subtitle",
                        "id": sub.id,
                        "subtitle": sub.text,
                        "secondary_subtitle": sub.secondary_text,
                        "sub_start": sub.sub_start,
                        "sub_end": sub.sub_end,
                        "media_path": sub.media_path,
                    }),
                    SubtitleEvent::SecondaryUpdate { id, text } => serde_json::json!({
                        "type": "secondary_update",
                        "id": id,
                        "secondary_subtitle": text,
                    }),
                    SubtitleEvent::SecondaryAppend { id, text } => serde_json::json!({
                        "type": "secondary_append",
                        "id": id,
                        "secondary_subtitle": text,
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
                    if let Some(eid) = end_id {
                        if let Some(end_sub) = store.get(&eid) {
                            sub.sub_end = end_sub.sub_end;
                        }
                    }
                    drop(store);
                    (id, "thumbnail", FfmpegRequest::thumbnail(&sub, image_config))
                }
                ProtocolRequest::Audio { id, offset_start, offset_end, audio_config } => {
                    let store = state.subtitles.read().await;
                    let sub = store.get(&id)?.clone();
                    drop(store);
                    (id, "audio", FfmpegRequest::audio(&sub, offset_start, offset_end, audio_config))
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
                warn!("[media] Failed to generate {} for subtitle {}", req_type, subtitle_id);
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
