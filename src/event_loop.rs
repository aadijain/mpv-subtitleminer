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

/// Drops subtitle lines based on their ASS Dialogue metadata fields.
#[derive(Clone, Default)]
pub struct StyleFilter {
    /// Drop lines whose Style field matches any of these entries.
    pub style_blocklist: Vec<String>,
    /// If non-empty, drop lines whose Style field does NOT match any entry.
    pub style_allowlist: Vec<String>,
    /// Drop lines whose Name field exactly matches any of these entries.
    pub name_blocklist: Vec<String>,
    /// If non-empty, drop lines whose Name field does NOT match any entry.
    pub name_allowlist: Vec<String>,
}

impl StyleFilter {
    fn is_active(&self) -> bool {
        !self.style_blocklist.is_empty()
            || !self.style_allowlist.is_empty()
            || !self.name_blocklist.is_empty()
            || !self.name_allowlist.is_empty()
    }

    /// Applies per-line filtering to `ass_full` (newline-separated Dialogue entries from mpv).
    /// Returns the plain-text survivors from `sub_text` joined by `\n`, or `None` if every
    /// parseable Dialogue line was dropped. Unparseable lines and non-ASS tracks pass through.
    fn filter(&self, ass_full: &str, sub_text: &str) -> Option<String> {
        let text_lines: Vec<&str> = sub_text.lines().collect();
        let mut kept: Vec<String> = Vec::new();
        let mut any_parsed = false;
        let mut cursor = 0usize;

        for dialogue in ass_full.lines() {
            match parse_ass_dialogue(dialogue) {
                None => {
                    if let Some(line) = text_lines.get(cursor) {
                        kept.push((*line).to_string());
                    }
                    cursor += 1;
                }
                Some((style, name, text)) => {
                    any_parsed = true;
                    let n = ass_visible_line_count(text);
                    let end = (cursor + n).min(text_lines.len());
                    if !self.line_should_drop(style, name) {
                        kept.push(text_lines[cursor..end].join("\n"));
                    }
                    cursor = end;
                }
            }
        }

        match (any_parsed, kept.is_empty()) {
            (false, _) => Some(sub_text.to_string()), // no ASS metadata → pass through
            (true, true) => None,                      // all lines filtered
            (true, false) => Some(kept.join("\n")),
        }
    }

    fn line_should_drop(&self, style: &str, name: &str) -> bool {
        if !self.style_allowlist.is_empty() && !self.style_allowlist.iter().any(|s| s == style) {
            return true;
        }
        if self.style_blocklist.iter().any(|s| s == style) {
            return true;
        }
        if !self.name_allowlist.is_empty() && !self.name_allowlist.iter().any(|n| n == name) {
            return true;
        }
        if self.name_blocklist.iter().any(|n| n == name) {
            return true;
        }
        false
    }
}

/// Extracts (Style, Name, Text) from a single ASS Dialogue line:
/// `[Dialogue: ]Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text`
fn parse_ass_dialogue(s: &str) -> Option<(&str, &str, &str)> {
    let s = s.strip_prefix("Dialogue: ").unwrap_or(s);
    let mut parts = s.splitn(10, ',');
    parts.next()?; // Layer
    parts.next()?; // Start
    parts.next()?; // End
    let style = parts.next()?.trim();
    let name = parts.next()?.trim();
    parts.next()?; // MarginL
    parts.next()?; // MarginR
    parts.next()?; // MarginV
    parts.next()?; // Effect
    let text = parts.next()?;
    Some((style, name, text))
}

/// Number of visible lines the ASS dialogue text renders as. Each `\N` (hard
/// newline) adds a line; `{...}` override blocks are stripped first so any
/// `\N` inside them doesn't count.
fn ass_visible_line_count(text: &str) -> usize {
    let mut stripped = String::with_capacity(text.len());
    let mut in_override = false;
    for c in text.chars() {
        match c {
            '{' => in_override = true,
            '}' => in_override = false,
            _ if !in_override => stripped.push(c),
            _ => {}
        }
    }
    1 + stripped.matches("\\N").count()
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
    // indices: 0=sub_start, 1=sub_end, 2=path, 3=aid, 4=sub_delay, 5=sub-text/ass-full (optional)
    responses: [Option<serde_json::Value>; 6],
}

impl PendingPrimary {
    fn new(id: u64, text: String, query_ass_full: bool) -> Self {
        let mut responses: [Option<serde_json::Value>; 6] = Default::default();
        if !query_ass_full {
            // Pre-fill the ass-full slot so is_ready() doesn't wait for a query we never sent.
            responses[5] = Some(serde_json::Value::Null);
        }
        Self { id, text, responses }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 6 {
            self.responses[index] = Some(value);
        }
    }

    fn is_ready(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    /// The raw `sub-text/ass-full` Dialogue line, if mpv returned a string for it.
    /// Returns None when the query was skipped or mpv returned an error/non-string.
    fn ass_full(&self) -> Option<&str> {
        self.responses[5].as_ref().and_then(|v| v.as_str())
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
    // indices: 0=secondary-sub-start, 1=secondary-sub-end, 2=secondary-sub-delay,
    //          3=secondary-sub-text/ass-full (optional)
    responses: [Option<serde_json::Value>; 4],
}

impl PendingSecondary {
    fn new(text: String, query_ass_full: bool) -> Self {
        let mut responses: [Option<serde_json::Value>; 4] = Default::default();
        if !query_ass_full {
            responses[3] = Some(serde_json::Value::Null);
        }
        Self { text, responses }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 4 {
            self.responses[index] = Some(value);
        }
    }

    fn is_ready(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    fn ass_full(&self) -> Option<&str> {
        self.responses[3].as_ref().and_then(|v| v.as_str())
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
    append_secondary: bool,
    primary_filter: StyleFilter,
    secondary_filter: StyleFilter,
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
        if let Err(e) = handle_mpv(
            mpv,
            mpv_state,
            mpv_tx,
            secondary_match_threshold,
            append_secondary,
            primary_filter,
            secondary_filter,
        ).await {
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
    secondary_match_threshold: f64,
    append_secondary: bool,    
    primary_filter: StyleFilter,
    secondary_filter: StyleFilter,
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

    // A single secondary whose get_property batch resolved before any matching
    // primary. Held only to cover the response-ordering race; under normal
    // playback it is claimed by the next primary or replaced by the next
    // secondary.
    let mut orphan_secondary: Option<(u64, f64, f64)> = None;

    // The most recently emitted primary: (subtitle_id, sub_start, sub_end, last_secondary).
    // `last_secondary` is None until the first secondary match, then holds the
    // most recently emitted secondary piece — used both to decide Update vs
    // Append and to skip appends that repeat the prior piece (mpv re-reports
    // the same Dialogue when a neighboring one appears/expires).
    let mut last_primary: Option<(u64, f64, f64, Option<String>)> = None;

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
                    let mut primary = pending_primary.remove(&base_id).unwrap();
                    let subtitle_id = primary.id;

                    // Discard if timing is invalid (IPC error on sub-start/end).
                    if sub_start >= f64::MAX / 2.0 {
                        continue;
                    }

                    // Filter out ASS lines that match the style/name filter; keep survivors.
                    if let Some(ass_full) = primary.ass_full().map(str::to_owned) {
                        match primary_filter.filter(&ass_full, &primary.text) {
                            None => {
                                debug!("[sub:{}] dropped (style/name filter)", subtitle_id);
                                continue;
                            }
                            Some(filtered) => primary.text = filtered,
                        }
                    }

                    // Emit immediately — secondary arrives via SecondaryUpdate.
                    emit(primary, None, &state, &tx).await;

                    // Did a secondary land just before this primary's batch finished?
                    let claimed_orphan = orphan_secondary
                        .filter(|(_, ss, se)| {
                            ranges_overlap(sub_start, sub_end, *ss, *se, secondary_match_threshold)
                        });
                    let mut last_secondary: Option<String> = None;
                    if let Some((s_base, _, _)) = claimed_orphan {
                        orphan_secondary = None;
                        if let Some(sec) = pending_secondary.remove(&s_base) {
                            if !sec.text.is_empty() {
                                let text = sec.text.clone();
                                let _ = tx.send(SubtitleEvent::SecondaryUpdate { id: subtitle_id, text });
                                last_secondary = Some(sec.text);
                            }
                        }
                    }

                    last_primary = Some((subtitle_id, sub_start, sub_end, last_secondary));
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

                    // Filter out ASS lines that match the style/name filter; keep survivors.
                    if let Some(ass_full) = pending_secondary[&base_id].ass_full().map(str::to_owned) {
                        let sub_text = pending_secondary[&base_id].text.clone();
                        match secondary_filter.filter(&ass_full, &sub_text) {
                            None => {
                                pending_secondary.remove(&base_id);
                                debug!("Dropped secondary (style/name filter)");
                                continue;
                            }
                            Some(filtered) => pending_secondary.get_mut(&base_id).unwrap().text = filtered,
                        }
                    }

                    let s_end = pending_secondary[&base_id].sub_end().unwrap_or(s_start);

                    let overlaps_last = last_primary
                        .as_ref()
                        .map(|(_, p_start, p_end, _)| {
                            ranges_overlap(*p_start, *p_end, s_start, s_end, secondary_match_threshold)
                        })
                        .unwrap_or(false);

                    if overlaps_last {
                        let (prim_id, _, _, last_secondary) = last_primary.as_mut().unwrap();
                        let prim_id = *prim_id;
                        let already_matched = last_secondary.is_some();

                        let sec_text_peek = pending_secondary[&base_id].text.clone();
                        if sec_text_peek.is_empty() {
                            // Mark the primary as claimed so a later overlapping
                            // secondary uses the append path rather than emitting as
                            // a first-time Update.
                            if last_secondary.is_none() {
                                *last_secondary = Some(String::new());
                            }
                            pending_secondary.remove(&base_id);
                            continue;
                        }

                        if already_matched {
                            // Would-be append: prefer letting an in-flight primary claim
                            // this instead. Park as orphan; primary-ready path consumes it.
                            if !pending_primary.is_empty() {
                                if let Some((prev_base, _, _)) = orphan_secondary.replace((base_id, s_start, s_end)) {
                                    if prev_base != base_id {
                                        pending_secondary.remove(&prev_base);
                                        debug!("Dropped unmatched secondary subtitle");
                                    }
                                }
                                debug!("[sub:{}] Secondary held for in-flight primary", prim_id);
                                continue;
                            }

                            // Dedup: mpv re-reports the same Dialogue when a neighbor
                            // appears/expires — skip if the piece matches what we last sent.
                            if last_secondary.as_deref() == Some(sec_text_peek.as_str()) {
                                pending_secondary.remove(&base_id);
                                debug!("[sub:{}] Secondary append skipped (duplicate)", prim_id);
                                continue;
                            }

                            let sec = pending_secondary.remove(&base_id).unwrap();
                            if append_secondary {
                                debug!("[sub:{}] Secondary append", prim_id);
                                let _ = tx.send(SubtitleEvent::SecondaryAppend { id: prim_id, text: sec.text.clone() });
                                *last_secondary = Some(sec.text);
                            } else {
                                debug!("[sub:{}] Secondary append suppressed (flag off)", prim_id);
                            }
                        } else {
                            let sec = pending_secondary.remove(&base_id).unwrap();
                            debug!("[sub:{}] Secondary match", prim_id);
                            let _ = tx.send(SubtitleEvent::SecondaryUpdate { id: prim_id, text: sec.text.clone() });
                            *last_secondary = Some(sec.text);
                        }
                    } else {
                        // No matching primary yet — hold for the upcoming one.
                        // Only one orphan slot; replace any existing entry.
                        if let Some((prev_base, _, _)) = orphan_secondary.replace((base_id, s_start, s_end)) {
                            if prev_base != base_id {
                                pending_secondary.remove(&prev_base);
                                debug!("Dropped unmatched secondary subtitle");
                            }
                        }
                    }
                }
            }

            continue;
        }

        // ── Seek: drop everything tied to the pre-seek timeline ───────────────
        // mpv fires `seek` at the start of the jump. Any in-flight get_property
        // responses still en route belong to the old position; clearing the
        // pending maps lets them fall through the dispatch below as no-ops.
        if json.get("event") == Some(&serde_json::json!("seek")) {
            pending_primary.clear();
            pending_secondary.clear();
            orphan_secondary = None;
            last_primary = None;
            debug!("Seek detected, cleared subtitle match state");
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

            let mut cmd = format!(
                concat!(
                    "{{\"command\":[\"get_property\",\"secondary-sub-start\"],\"request_id\":{0}}}\n",
                    "{{\"command\":[\"get_property\",\"secondary-sub-end\"],\"request_id\":{1}}}\n",
                    "{{\"command\":[\"get_property\",\"secondary-sub-delay\"],\"request_id\":{2}}}\n"
                ),
                base_id,
                base_id + 1,
                base_id + 2
            );
            let query_ass_full = secondary_filter.is_active();
            if query_ass_full {
                cmd.push_str(&format!(
                    "{{\"command\":[\"get_property\",\"secondary-sub-text/ass-full\"],\"request_id\":{}}}\n",
                    base_id + 3
                ));
            }
            mpv.write_all(cmd.as_bytes()).await?;
            pending_secondary.insert(base_id, PendingSecondary::new(text, query_ass_full));
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

                let mut cmd = format!(
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
                let query_ass_full = primary_filter.is_active();
                if query_ass_full {
                    cmd.push_str(&format!(
                        "{{\"command\":[\"get_property\",\"sub-text/ass-full\"],\"request_id\":{}}}\n",
                        base_id + 5
                    ));
                }

                mpv.write_all(cmd.as_bytes()).await?;
                pending_primary.insert(
                    base_id,
                    PendingPrimary::new(subtitle_id, text.to_string(), query_ass_full),
                );
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
