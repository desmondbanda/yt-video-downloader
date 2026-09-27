#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

#[test]
fn cookie_options_reach_both_media_commands_as_single_arguments() {
    let fixture = Fixture::new();
    fixture.executable("ffmpeg", "exit 0");
    fixture.executable("ffprobe", "exit 0");
    let engine = fixture.executable(
        "yt-dlp",
        r#"
[ "$1" = '--ignore-config' ] || exit 10
[ "$2" = "$EXPECTED_FLAG" ] || exit 11
[ "$3" = "$EXPECTED_VALUE" ] || exit 12
printf '{"title":"Test","formats":[]}\n'
"#,
    );
    for command in ["info", "get"] {
        for (flag, value) in [
            ("--cookies-from-browser", "chrome:Profile 1"),
            ("--cookies", "/tmp/my cookies.txt"),
        ] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_ytd"));
            cmd.env("PATH", &fixture.0)
                .env("EXPECTED_FLAG", flag)
                .env("EXPECTED_VALUE", value)
                .arg("--yt-dlp")
                .arg(&engine)
                .args([command, "https://example.com/video", flag, value]);
            if command == "get" {
                cmd.arg("-o").arg(fixture.0.join("output"));
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn conflicting_cookie_sources_are_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_ytd"))
        .args([
            "info",
            "https://example.com/video",
            "--cookies",
            "cookies.txt",
            "--cookies-from-browser",
            "chrome",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ytd-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn executable(&self, name: &str, script: &str) -> std::path::PathBuf {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn download_preserves_quality_cap_and_treats_url_as_an_argument() {
    let fixture = Fixture::new();
    fixture.executable("ffmpeg", "exit 0");
    fixture.executable("ffprobe", "exit 0");
    let engine = fixture.executable(
        "yt-dlp",
        r#"
[ "$1" = '--ignore-config' ] || exit 10
seen_format=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        -f)
            shift
            [ "$1" = 'bv*[height<=480]+ba/b[height<=480]' ] || exit 11
            seen_format=1
            ;;
        --)
            shift
            [ "$#" -eq 1 ] || exit 12
            [ "$1" = '--not-an-option' ] || exit 13
            break
            ;;
    esac
    shift
done
[ "$seen_format" -eq 1 ] || exit 14
printf '@@P 10|20|NA|5|2\n@@F tutorial.mp4\n'
"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_ytd"))
        .env("PATH", &fixture.0)
        .arg("--yt-dlp")
        .arg(engine)
        .args(["get", "-q", "480", "-o"])
        .arg(fixture.0.join("output"))
        .args(["--", "--not-an-option"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("saved tutorial.mp4"));
}

#[test]
fn doctor_fails_when_an_installed_tool_returns_failure() {
    let fixture = Fixture::new();
    let engine = fixture.executable("yt-dlp", "echo test-version");
    fixture.executable("deno", "exit 1");
    fixture.executable("ffmpeg", "exit 0");
    fixture.executable("ffprobe", "exit 0");
    let output = Command::new(env!("CARGO_BIN_EXE_ytd"))
        .env("PATH", &fixture.0)
        .arg("--yt-dlp")
        .arg(engine)
        .arg("doctor")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("MISSING  deno"));
}

#[test]
fn zero_quality_is_rejected_before_running_engine() {
    let output = Command::new(env!("CARGO_BIN_EXE_ytd"))
        .args(["get", "https://example.com/video", "-q", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
