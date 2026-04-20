mod event_loop;
mod media;
mod mpv_stream;

use clap::Parser;
use event_loop::{StyleFilter, run_server};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the mpv IPC socket
    #[cfg_attr(unix, arg(default_value = "/tmp/mpv-socket"))]
    #[cfg_attr(windows, arg(default_value = r"\\.\pipe\mpv-socket"))]
    socket_path: String,

    /// WebSocket server port
    #[arg(default_value_t = 61777)]
    port: u16,

    /// Path to ffmpeg binary
    #[arg(default_value = "ffmpeg")]
    ffmpeg_path: String,

    /// Validate that the IPC socket belongs to this mpv PID
    #[arg(long)]
    expected_mpv_pid: Option<u32>,

    /// Tolerance (in seconds) applied to the overlap check when matching primary
    /// and secondary subtitle lines. Two lines match if their time ranges overlap
    /// within this many seconds. (default: 0.5)
    #[arg(long, default_value_t = 0.5)]
    secondary_match_threshold: f64,

    /// Comma-separated ASS Style values to drop on the primary subtitle track.
    #[arg(long, default_value = "")]
    style_blocklist_primary: String,

    /// Comma-separated ASS Style values to keep on the primary subtitle track.
    /// Lines with any other style are dropped. Empty disables allowlist filtering.
    /// (default: "Default")
    #[arg(long, default_value = "Default")]
    style_allowlist_primary: String,

    /// Comma-separated ASS Name field values to drop on the primary subtitle track.
    #[arg(long, default_value = "")]
    name_blocklist_primary: String,

    /// Comma-separated ASS Name field values to keep on the primary subtitle track.
    /// Lines with any other name are dropped. Empty disables allowlist filtering.
    #[arg(long, default_value = "")]
    name_allowlist_primary: String,

    /// Comma-separated ASS Style values to drop on the secondary subtitle track.
    #[arg(long, default_value = "")]
    style_blocklist_secondary: String,

    /// Comma-separated ASS Style values to keep on the secondary subtitle track.
    /// Lines with any other style are dropped. Empty disables allowlist filtering.
    /// (default: "Default")
    #[arg(long, default_value = "Default")]
    style_allowlist_secondary: String,

    /// Comma-separated ASS Name field values to drop on the secondary subtitle track.
    #[arg(long, default_value = "")]
    name_blocklist_secondary: String,

    /// Comma-separated ASS Name field values to keep on the secondary subtitle track.
    /// Lines with any other name are dropped. Empty disables allowlist filtering.
    #[arg(long, default_value = "")]
    name_allowlist_secondary: String,
}

fn parse_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    media::init_ffmpeg_path(&args.ffmpeg_path);
    log::info!("Using ffmpeg: {}", args.ffmpeg_path);

    let primary_filter = StyleFilter {
        style_blocklist: parse_list(&args.style_blocklist_primary),
        style_allowlist: parse_list(&args.style_allowlist_primary),
        name_blocklist: parse_list(&args.name_blocklist_primary),
        name_allowlist: parse_list(&args.name_allowlist_primary),
    };
    let secondary_filter = StyleFilter {
        style_blocklist: parse_list(&args.style_blocklist_secondary),
        style_allowlist: parse_list(&args.style_allowlist_secondary),
        name_blocklist: parse_list(&args.name_blocklist_secondary),
        name_allowlist: parse_list(&args.name_allowlist_secondary),
    };

    if let Err(e) = run_server(
        &args.socket_path,
        args.port,
        args.expected_mpv_pid,
        args.secondary_match_threshold,
        primary_filter,
        secondary_filter,
    ).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}