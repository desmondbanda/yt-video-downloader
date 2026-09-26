# ytd: a YouTube downloader

A small Rust command-line app around [yt-dlp](https://github.com/yt-dlp/yt-dlp). yt-dlp handles extraction and downloading; this app provides quality limits, rough size estimates, progress bars and engine updates.

## Setup (macOS)

You need Rust/Cargo and Homebrew. Cargo builds this app and fetches its Rust libraries automatically. The external tools are separate:

| Tool | Purpose |
| --- | --- |
| yt-dlp | Finds video/audio streams and downloads them |
| Deno | Runs YouTube JavaScript challenge solvers |
| FFmpeg + ffprobe | Merge streams, inspect media and convert audio; both come with the FFmpeg package |

From this project directory:

```sh
brew install yt-dlp deno ffmpeg
cargo install --path . --locked
ytd doctor
```

`--path .` means “install the Rust project in this directory”. This installs the executable named `ytd` into `~/.cargo/bin`; it does not install an additional package called “path”. `doctor` is a subcommand of `ytd`, not a separate dependency. It exits unsuccessfully if a required tool is missing or fails its version check.

If `ytd` is not found, put `~/.cargo/bin` on your PATH, or use it directly:

```sh
~/.cargo/bin/ytd doctor
```

You can also run without installing:

```sh
cargo run --locked -- doctor
cargo run --locked -- get 'https://www.youtube.com/watch?v=VIDEO_ID' -q 480
```

Deno is enabled by default in yt-dlp. You also need the yt-dlp EJS challenge scripts; official standalone releases and the Homebrew package provide these. For a pip installation, use `yt-dlp[default]` in the intended Python environment. See the [official EJS setup guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS). `doctor` checks executables locally; it does not verify YouTube connectivity or successful challenge solving.

## Usage

Quote URLs, especially ones containing `&` or `?`, so the shell passes them intact.

```sh
ytd info 'https://www.youtube.com/watch?v=VIDEO_ID'
ytd get 'https://www.youtube.com/watch?v=VIDEO_ID'          # up to 720p, MP4
ytd get 'https://www.youtube.com/watch?v=VIDEO_ID' -q 480   # lower resolution
ytd get 'https://www.youtube.com/watch?v=VIDEO_ID' -a m4a  # audio only
ytd get 'https://www.youtube.com/watch?v=VIDEO_ID' -a mp3
ytd get 'https://www.youtube.com/playlist?list=PLAYLIST_ID' --playlist
ytd get 'https://www.youtube.com/watch?v=VIDEO_ID' -o './videos'
ytd update
```

Files default to `~/Downloads/YouTube`. Video downloads obey the requested height limit; if no suitable format exists, the command fails instead of fetching a higher resolution. MP4 is a container, and compatibility still depends on the codecs your player supports. Remuxing changes the container without re-encoding.

`info` uses internet data to retrieve metadata but does not download the video. Its estimates compare available stream sizes and include audio only once; missing sizes display as unknown. These are rough estimates, not a bundle budget or a measurement of all network traffic. Audio conversion changes the saved file size, not the bytes already downloaded.

Interrupted downloads can normally resume when you repeat the same command with its partial files still present. Existing matching output files are normally reused, but this app has no permanent download history: changing the title, output directory or format can cause another download. Playlist mode downloads every available entry, so use it deliberately on a limited bundle.

## Maintenance and development

`ytd update` uses yt-dlp's self-updater and falls back to Homebrew when yt-dlp identifies a Homebrew installation. Python installations get instructions to update in their original environment. To update the Homebrew tools together:

```sh
brew upgrade yt-dlp deno ffmpeg
```

After changing this app, rerun `cargo install --path . --locked` to replace the installed `ytd`.

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

`src/main.rs` handles CLI arguments and terminal output. `src/engine.rs` builds subprocess commands and parses metadata/progress. This separation is sufficient for the current app; no web framework, database or async runtime is needed.

The wrapper ignores global yt-dlp configuration so it cannot silently change download settings. Select a different executable using `ytd --yt-dlp /path/to/yt-dlp doctor` or `YTD_YT_DLP`.
