//! Thin wrapper around the yt-dlp binary. All YouTube extraction is delegated
//! to yt-dlp; this module only builds command lines and parses its output.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const PROGRESS_PREFIX: &str = "@@P ";
const FILE_PREFIX: &str = "@@F ";

pub struct Engine {
    bin: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct VideoInfo {
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub formats: Vec<Format>,
}

#[derive(Debug, Deserialize)]
pub struct Format {
    pub height: Option<u32>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    /// Total bitrate in kbit/s.
    pub tbr: Option<f64>,
}

impl Format {
    fn has_video(&self) -> bool {
        self.vcodec.as_deref().is_some_and(|c| c != "none")
    }

    fn has_audio(&self) -> bool {
        self.acodec.as_deref().is_some_and(|c| c != "none")
    }

    fn size(&self) -> Option<u64> {
        self.filesize.or(self.filesize_approx)
    }

    /// Reported size, or bitrate x duration when YouTube gives no size.
    fn size_or_estimate(&self, duration: Option<f64>) -> Option<u64> {
        self.size()
            .or_else(|| Some((self.tbr? * 1000.0 / 8.0 * duration?) as u64))
    }
}

/// Formats yt-dlp would realistically pick: those with a reported size
/// (the https ones), falling back to all of them if none report one.
fn sized_first<'a>(formats: Vec<&'a Format>) -> Vec<&'a Format> {
    let sized: Vec<_> = formats
        .iter()
        .copied()
        .filter(|f| f.size().is_some())
        .collect();
    if sized.is_empty() { formats } else { sized }
}

impl VideoInfo {
    /// Rough upper estimate across known formats at each resolution.
    /// Combined streams already include audio; incomplete sizes stay unknown.
    pub fn size_estimates(&self) -> (Vec<(u32, Option<u64>)>, Option<u64>) {
        let audio = sized_first(
            self.formats
                .iter()
                .filter(|f| f.has_audio() && !f.has_video())
                .collect(),
        )
        .into_iter()
        .map(|f| f.size_or_estimate(self.duration))
        .collect::<Option<Vec<_>>>()
        .and_then(|sizes| sizes.into_iter().max());

        let mut heights: Vec<u32> = self
            .formats
            .iter()
            .filter(|f| f.has_video())
            .filter_map(|f| f.height)
            .collect();
        heights.sort_unstable();
        heights.dedup();

        let rows = heights
            .into_iter()
            .map(|h| {
                let video = sized_first(
                    self.formats
                        .iter()
                        .filter(|f| f.has_video() && f.height == Some(h))
                        .collect(),
                )
                .into_iter()
                .map(|f| {
                    let size = f.size_or_estimate(self.duration)?;
                    if f.has_audio() {
                        Some(size)
                    } else {
                        size.checked_add(audio?)
                    }
                })
                .collect::<Option<Vec<_>>>()
                .and_then(|sizes| sizes.into_iter().max());
                (h, video)
            })
            .collect();
        (rows, audio)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AudioFormat {
    Mp3,
    M4a,
}

pub enum Quality {
    /// Best video up to this height, merged with best audio.
    Video {
        max_height: u32,
    },
    AudioOnly(AudioFormat),
}

pub struct DownloadRequest<'a> {
    pub url: &'a str,
    pub quality: Quality,
    pub out_dir: &'a Path,
    pub allow_playlist: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: Option<f64>,
    pub eta: Option<u64>,
}

pub enum Event {
    Progress(Progress),
    /// A status line from yt-dlp, e.g. "[Merger] Merging formats into ...".
    Status(String),
    /// Final path of a finished file (after merging/conversion).
    Finished(PathBuf),
}

impl Engine {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.bin);
        // User-wide configuration must not silently alter download behavior.
        command.arg("--ignore-config");
        command
    }

    pub fn version(&self) -> Result<String> {
        let out = self
            .command()
            .arg("--version")
            .output()
            .with_context(|| missing_bin_hint(&self.bin))?;
        if !out.status.success() {
            bail!(
                "yt-dlp version check failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if version.is_empty() {
            bail!("yt-dlp returned an empty version");
        }
        Ok(version)
    }

    pub fn probe(&self, url: &str) -> Result<VideoInfo> {
        let out = self
            .command()
            .args(["--dump-single-json", "--no-playlist", "--", url])
            .stderr(Stdio::inherit())
            .output()
            .with_context(|| missing_bin_hint(&self.bin))?;
        if !out.status.success() {
            bail!("yt-dlp could not read that URL");
        }
        serde_json::from_slice(&out.stdout).context("unexpected JSON from yt-dlp")
    }

    pub fn download(&self, req: &DownloadRequest, mut on_event: impl FnMut(Event)) -> Result<()> {
        let mut cmd = self.command();
        cmd.arg(if req.allow_playlist {
            "--yes-playlist"
        } else {
            "--no-playlist"
        })
        .args(["--newline", "--progress", "--progress-template"])
        .arg(format!(
            "download:{PROGRESS_PREFIX}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|\
                 %(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s"
        ))
        .args(["--print", &format!("after_move:{FILE_PREFIX}%(filepath)s")])
        .arg("-P")
        .arg(req.out_dir)
        .args(["-o", "%(title)s [%(id)s].%(ext)s"]);

        match req.quality {
            Quality::Video { max_height } => {
                cmd.arg("-f")
                    .arg(format!(
                        "bv*[height<={max_height}]+ba/b[height<={max_height}]"
                    ))
                    .args(["--merge-output-format", "mp4", "--remux-video", "mp4"]);
            }
            Quality::AudioOnly(AudioFormat::Mp3) => {
                cmd.args([
                    "-f",
                    "ba/b",
                    "-x",
                    "--audio-format",
                    "mp3",
                    "--audio-quality",
                    "0",
                ]);
            }
            Quality::AudioOnly(AudioFormat::M4a) => {
                cmd.args(["-f", "ba[ext=m4a]/ba/b", "-x", "--audio-format", "m4a"]);
            }
        }

        let mut child = cmd
            .arg("--")
            .arg(req.url)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| missing_bin_hint(&self.bin))?;

        let stdout = child.stdout.take().expect("stdout is piped");
        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error).context("could not read yt-dlp output");
                }
            };
            if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
                on_event(Event::Progress(parse_progress(rest)));
            } else if let Some(path) = line.strip_prefix(FILE_PREFIX) {
                on_event(Event::Finished(PathBuf::from(path)));
            } else if line.starts_with("[Merger]") || line.starts_with("[ExtractAudio]") {
                on_event(Event::Status(line));
            }
        }

        let status = child.wait()?;
        if !status.success() {
            bail!("yt-dlp exited with {status}");
        }
        Ok(())
    }

    /// Update the engine. The standalone binary can update itself with `-U`;
    /// Homebrew installs fall back to brew; Python installs need their own environment.
    pub fn update(&self) -> Result<()> {
        let out = self
            .command()
            .arg("-U")
            .output()
            .with_context(|| missing_bin_hint(&self.bin))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        print!("{text}");

        if out.status.success() {
            return Ok(());
        }
        let lower = text.to_lowercase();
        if lower.contains("brew") {
            println!("Updating through Homebrew instead...");
            run_passthrough(Command::new("brew").args(["upgrade", "yt-dlp"]))
        } else if lower.contains("pip") {
            bail!(
                "Update yt-dlp using the Python environment or package manager that installed it (pip, pipx or uv). Automatic pip updates may target the wrong environment."
            )
        } else {
            bail!("yt-dlp could not update itself")
        }
    }
}

fn run_passthrough(cmd: &mut Command) -> Result<()> {
    let status = cmd.status()?;
    if !status.success() {
        bail!("update command exited with {status}");
    }
    Ok(())
}

fn parse_progress(s: &str) -> Progress {
    let mut parts = s
        .split('|')
        .map(|p| p.trim())
        .map(|p| p.parse::<f64>().ok());
    let mut next = || parts.next().flatten();
    let downloaded = next().unwrap_or(0.0) as u64;
    let total = next();
    let estimate = next();
    Progress {
        downloaded,
        total: total.or(estimate).map(|t| t as u64),
        speed: next(),
        eta: next().map(|e| e as u64),
    }
}

fn missing_bin_hint(bin: &Path) -> String {
    format!(
        "could not run `{}`. Install it with `brew install yt-dlp deno ffmpeg`, then run `ytd doctor`",
        bin.display()
    )
}

/// Returns whether `name` can be launched from PATH.
pub fn tool_available(name: &str, version_flag: &str) -> bool {
    Command::new(name)
        .arg(version_flag)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimates(formats: serde_json::Value) -> (Vec<(u32, Option<u64>)>, Option<u64>) {
        let info: VideoInfo = serde_json::from_value(serde_json::json!({
            "title": "Tutorial", "formats": formats
        }))
        .unwrap();
        info.size_estimates()
    }

    #[test]
    fn combined_stream_does_not_count_audio_twice() {
        let (rows, audio) = estimates(serde_json::json!([
            {"height": 720, "vcodec": "h264", "acodec": "aac", "filesize": 100},
            {"vcodec": "none", "acodec": "aac", "filesize": 20}
        ]));
        assert_eq!(rows, vec![(720, Some(100))]);
        assert_eq!(audio, Some(20));
    }

    #[test]
    fn separate_streams_include_audio_bytes() {
        let (rows, _) = estimates(serde_json::json!([
            {"height": 480, "vcodec": "h264", "acodec": "none", "filesize_approx": 80},
            {"vcodec": "none", "acodec": "aac", "filesize": 20}
        ]));
        assert_eq!(rows, vec![(480, Some(100))]);
    }

    #[test]
    fn missing_audio_size_is_not_treated_as_zero() {
        let (rows, audio) = estimates(serde_json::json!([
            {"height": 720, "vcodec": "h264", "acodec": "none", "filesize": 100},
            {"vcodec": "none", "acodec": "aac"}
        ]));
        assert_eq!(rows, vec![(720, None)]);
        assert_eq!(audio, None);
    }

    #[test]
    fn unsized_stream_formats_do_not_hide_known_sizes() {
        let (rows, _) = estimates(serde_json::json!([
            {"height": 720, "vcodec": "h264", "acodec": "none", "filesize": 100},
            {"height": 720, "vcodec": "h264", "acodec": "none", "tbr": 9000.0},
            {"vcodec": "none", "acodec": "aac", "filesize": 20}
        ]));
        assert_eq!(rows, vec![(720, Some(120))]);
    }

    #[test]
    fn falls_back_to_bitrate_times_duration() {
        let info: VideoInfo = serde_json::from_value(serde_json::json!({
            "title": "Live", "duration": 10.0,
            "formats": [{"height": 360, "vcodec": "h264", "acodec": "aac", "tbr": 800.0}]
        }))
        .unwrap();
        assert_eq!(info.size_estimates().0, vec![(360, Some(1_000_000))]);
    }

    #[cfg(unix)]
    #[test]
    fn failed_executable_is_not_healthy() {
        assert!(!tool_available("/usr/bin/false", "--version"));
        assert!(Engine::new("/usr/bin/false").version().is_err());
    }

    #[test]
    fn parses_progress_with_missing_fields() {
        let p = parse_progress("130048|309288|NA|603522.08|3");
        assert_eq!(p.downloaded, 130048);
        assert_eq!(p.total, Some(309288));
        assert_eq!(p.eta, Some(3));

        let p = parse_progress("5000|NA|90000|NA|NA");
        assert_eq!(p.total, Some(90000));
        assert_eq!(p.speed, None);
    }
}
