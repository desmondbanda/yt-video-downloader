# Future idea: Ratatui terminal interface

Memo added September 27, 2026.

Consider building an interactive terminal interface with **Ratatui** once the
current downloader works reliably. This is a future idea, not a prerequisite
for using or improving the CLI.

- [ ] Verify video downloads, audio playback, and resuming interrupted downloads.
- [ ] Explore an optional `ytd tui` command using Ratatui.
- [ ] Add a URL input, quality/audio selection, and output-folder selection.
- [ ] Show a download queue with progress, completion status, and useful errors.

Reuse the download engine in `src/engine.rs` and keep the existing CLI commands
for quick downloads and scripts. The interface can be rebuilt without rewriting
the yt-dlp integration.
