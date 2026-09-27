mod engine;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};

use engine::{AudioFormat, DownloadRequest, Engine, Event, Quality};

const DOWNLOAD_HELP: &str = "\
Examples:
  Download video with audio at up to 720p HD using Chrome cookies:
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo&list=PLqbS7AVVErFiWDOAVrPt7aYmnuuOLYvOa&index=4' -q 720 --cookies-from-browser chrome

  Using Firefox or Safari instead:
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' -q 720 --cookies-from-browser firefox
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' -q 720 --cookies-from-browser safari

  Download without browser cookies:
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' -q 1080

  Check available picture qualities and estimated download sizes:
    ytd info 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' --cookies-from-browser chrome

  Save audio only as MP3:
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' -a mp3

  Choose a destination folder:
    ytd get 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' -q 480 -o './videos'

How the options work:
  -q, --quality sets the maximum picture height in pixels:
    360 = 360p, 480 = 480p SD, 720 = 720p HD, 1080 = 1080p Full HD.
    The default is 720. Lower quality generally uses less data.
    A lower resolution may be selected if the requested quality is unavailable.
  Video downloads include audio and are saved as MP4 by default.
  -a, --audio saves only audio; -q does not apply to audio-only downloads.
  --cookies-from-browser uses the named browser's session for authentication.
    If YouTube asks you to sign in, open the video in that browser, sign in,
    and complete any verification before retrying. Cookies may help but do
    not guarantee access. Browser cookies are only read when you request them.
  Files are saved in ~/Downloads/YouTube unless you set -o, --output.
  A video URL containing &list= still downloads only that video.
    Add --playlist only when you want every available video in the playlist.
  Keep URLs in quotes, as shown above, so the shell handles & and ? correctly.";

/// Download YouTube videos (via yt-dlp) so you only pay for the data once.
#[derive(Parser)]
#[command(name = "ytd", version, after_help = DOWNLOAD_HELP)]
struct Cli {
    /// Path to the yt-dlp binary
    #[arg(long, global = true, default_value = "yt-dlp", env = "YTD_YT_DLP")]
    yt_dlp: PathBuf,

    /// Use a browser session, e.g. chrome, firefox, safari, or chrome:PROFILE
    #[arg(long, global = true, conflicts_with = "cookies")]
    cookies_from_browser: Option<String>,

    /// Read cookies from a Netscape-format cookie file
    #[arg(long, global = true)]
    cookies: Option<PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show a video's title and the approximate size at each quality
    #[command(
        after_help = "Example:\n  ytd info 'https://www.youtube.com/watch?v=8O0Nt9qY_vo' --cookies-from-browser chrome\n\nShows available picture qualities and rough download sizes without downloading.\nUse the browser where you signed into YouTube; firefox and safari also work."
    )]
    Info { url: String },
    /// Download a video (or just its audio)
    #[command(after_help = DOWNLOAD_HELP)]
    Get {
        url: String,
        /// Maximum picture quality: 480 = SD, 720 = HD, 1080 = Full HD. Includes audio.
        #[arg(short, long, default_value_t = 720, value_parser = clap::value_parser!(u32).range(1..))]
        quality: u32,
        /// Download audio only, in this format
        #[arg(short, long, value_enum)]
        audio: Option<AudioArg>,
        /// Where to save files (default: ~/Downloads/YouTube)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Download every video if the URL is a playlist
        #[arg(long)]
        playlist: bool,
    },
    /// Update the yt-dlp engine. Run this whenever downloads start failing.
    Update,
    /// Check that yt-dlp, deno and ffmpeg are installed
    Doctor,
}

#[derive(Clone, Copy, ValueEnum)]
enum AudioArg {
    Mp3,
    M4a,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let engine = Engine::new(cli.yt_dlp).with_cookies(cli.cookies_from_browser, cli.cookies);

    match cli.command {
        Cmd::Info { url } => info(&engine, &url),
        Cmd::Get {
            url,
            quality,
            audio,
            output,
            playlist,
        } => {
            let quality = match audio {
                Some(AudioArg::Mp3) => Quality::AudioOnly(AudioFormat::Mp3),
                Some(AudioArg::M4a) => Quality::AudioOnly(AudioFormat::M4a),
                None => Quality::Video {
                    max_height: quality,
                },
            };
            let out_dir = output.unwrap_or_else(default_out_dir);
            std::fs::create_dir_all(&out_dir)?;
            get(
                &engine,
                &DownloadRequest {
                    url: &url,
                    quality,
                    out_dir: &out_dir,
                    allow_playlist: playlist,
                },
            )
        }
        Cmd::Update => engine.update(),
        Cmd::Doctor => doctor(&engine),
    }
}

fn info(engine: &Engine, url: &str) -> Result<()> {
    let info = engine.probe(url)?;
    println!("{}", info.title);
    if let Some(uploader) = &info.uploader {
        println!("by {uploader}");
    }
    if let Some(secs) = info.duration {
        let secs = secs as u64;
        println!("length {}:{:02}", secs / 60, secs % 60);
    }

    let (rows, audio) = info.size_estimates();
    println!("\nApproximate download size:");
    for (height, size) in rows {
        println!("  {:>5}p  {}", height, fmt_size(size));
    }
    println!("  audio   {}", fmt_size(audio));
    Ok(())
}

fn get(engine: &Engine, req: &DownloadRequest) -> Result<()> {
    if !engine::tool_available("ffmpeg", "-version")
        || !engine::tool_available("ffprobe", "-version")
    {
        anyhow::bail!(
            "ffmpeg and ffprobe are required to merge video+audio and convert audio. Install it with `brew install ffmpeg`"
        );
    }

    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::with_template("{spinner} [{bar:30}] {bytes}/{total_bytes} {msg}")?
            .progress_chars("=> "),
    );
    bar.enable_steady_tick(Duration::from_millis(120));

    let mut finished = Vec::new();
    let result = engine.download(req, |event| match event {
        Event::Progress(p) => {
            // Video and audio arrive as separate streams; start a fresh bar
            // when the byte count goes backwards.
            if p.downloaded < bar.position() {
                bar.reset();
            }
            bar.set_length(p.total.unwrap_or(0));
            bar.set_position(p.downloaded);
            let speed = p
                .speed
                .map(|s| format!("{}/s", HumanBytes(s as u64)))
                .unwrap_or_default();
            let eta = p.eta.map(|e| format!("eta {e}s")).unwrap_or_default();
            bar.set_message(format!("{speed} {eta}"));
        }
        Event::Status(line) => bar.set_message(line),
        Event::Finished(path) => {
            finished.push(path);
        }
    });
    bar.finish_and_clear();
    for path in &finished {
        println!("saved {}", path.display());
    }
    result?;

    if finished.is_empty() {
        println!("Nothing new to download (already saved?)");
    }
    Ok(())
}

fn doctor(engine: &Engine) -> Result<()> {
    let mut ok = true;
    match engine.version() {
        Ok(v) => println!("ok       yt-dlp {v}"),
        Err(_) => {
            ok = false;
            println!("MISSING  yt-dlp  (does the extraction)");
        }
    }
    for (tool, flag, why) in [
        (
            "deno",
            "--version",
            "solves YouTube's JavaScript challenges",
        ),
        (
            "ffmpeg",
            "-version",
            "merges video+audio and converts to mp3",
        ),
        ("ffprobe", "-version", "inspects media for audio conversion"),
    ] {
        if engine::tool_available(tool, flag) {
            println!("ok       {tool}");
        } else {
            ok = false;
            println!("MISSING  {tool}  ({why})");
        }
    }
    if !ok {
        println!("\nInstall everything with:  brew install yt-dlp deno ffmpeg");
        anyhow::bail!("required tools are missing or failed their version checks");
    }
    Ok(())
}

fn default_out_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("YouTube")
}

fn fmt_size(size: Option<u64>) -> String {
    size.map(|s| format!("~{}", HumanBytes(s)))
        .unwrap_or_else(|| "unknown".into())
}
