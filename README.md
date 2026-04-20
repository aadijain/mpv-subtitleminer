# mpv-subtitleminer

> **Fork** of [friedrich-de/mpv-subtitleminer](https://github.com/friedrich-de/mpv-subtitleminer) with additional features — see below.

This is a tool for mpv to enable language learning with subtitle files. We send subtitles to a web-front end where you can look up words, create Anki flashcards add the sentence and media to your card. We do this by launching a local Rust server from mpv that connects the web front end to mpv via its IPC interface.

**Warning**: This is an early release. Expect bugs and rough edges.

## Demo

https://github.com/user-attachments/assets/47437d89-54f1-4039-bd17-d1fb8b453725

## Features

- Stream subtitles to web front end and interactively look up words.
- Anki integration via AnkiConnect: Select your note type, make a card and add media.
- Replay sentences with audio anytime.
- *(fork)* Secondary subtitle support: supports streaming mpv's secondary subtitles to the web front end. (secondary subs are automatically matched to the corresponding primary subs)
- *(fork)* Adjustable font size.
- *(fork)* Clear Screen button.
- *(fork)* Page title updates based on currently playing media file name.
- *(fork)* Parse subtitle text before exporting to Anki. (strip speaker names, annotations, etc.)
- *(fork)* Filter subtitle lines by ASS Style/Name field using per-track blocklists and allowlists.

## Downloads

Grab the matching `.zip` from the GitHub Releases page:

- **Linux (x86_64)**: `mpv-subtitleminer-x86_64-unknown-linux-gnu.zip`
- **macOS (Intel)**: `mpv-subtitleminer-x86_64-apple-darwin.zip`
- **macOS (Apple Silicon)**: `mpv-subtitleminer-aarch64-apple-darwin.zip`
- **Windows (x86_64)**: `mpv-subtitleminer-x86_64-pc-windows-msvc.zip`

## Setup

1. Install **mpv** (Windows: Best use `winget install mpv`).
2. Download the correct release `.zip` (see “Downloads”) and extract it.
3. Copy the extracted `mpv` folder into your mpv config directory:
   - **Windows**: `%APPDATA%\mpv\` (e.g. `C:\Users\YourName\AppData\Roaming\mpv\`).
     Create the folder if it doesn't exist.
   - **Linux/macOS**: `~/.config/mpv/`
   - **Flatpak**: `~/.var/app/io.mpv.Mpv/config/mpv/`
4. Merge the packaged `mpv.conf` into your existing config (make sure the `input-ipc-server` line is present).
5. Configure **AnkiConnect** to allow opening `index.html` from disk (file origin = `null`):

   Add `"null"` to `webCorsOriginList` so it looks like:

   ```json
   {
     "apiKey": null,
     "apiLogPath": null,
     "ignoreOriginList": [],
     "webBindAddress": "127.0.0.1",
     "webBindPort": 8765,
     "webCorsOriginList": ["http://localhost", "null"]
   }
   ```

   ![AnkiConnect CORS config example](img/cors.png)

## Usage

- The server starts automatically with mpv, simply open a video with subtitles.
- Open `index.html` in your browser. It should automatically connect to the running mpv instance.
- Press `Ctrl+a` to toggle/restart the server at any time.

## Troubleshooting

1. Connection errors: restart your browser.
2. Restart the server with `Ctrl+a`.
3. Ensure everything is at the correct place. The final folder structure should look like this:

```
mpv/
├── scripts/
│   └── mpv-subtitleminer.lua
├── script-opts/
│   └── mpv-subtitleminer.conf
├── mpv.conf
├── mpv-subtitleminer.exe
├── ffmpeg.exe (optional; if you don't have ffmpeg in PATH)
└── index.html

```

4. To change settings (ports, auto-start, secondary subtitle matching tolerance), edit `script-opts/mpv-subtitleminer.conf`.

5. Open mpv from the command line to see error messages. Press Ctrl+a to restart the server and check for errors.
