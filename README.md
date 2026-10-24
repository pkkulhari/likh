# Likh

Hold **Ctrl+Win**, speak, then release the keys to paste the final transcript into your current text input. Win is the Windows/Super key on Linux. Linux/X11 only. Rust application with a small Xlib bridge, using Claude's voice WebSocket service.

Likh runs as one background process. A small rounded pill shows a live waveform while listening, with bars that react to your microphone volume. Animated dots show microphone startup and processing; errors keep readable text. The overlay never takes focus. Audio streams during the hold; only final text is pasted. Escape cancels recording or pending transcription. Nothing presses Enter for you.

## Install or update

Run the same command for a first install or an update to the latest release:

```sh
curl -fsSL https://github.com/pkkulhari/likh/releases/latest/download/install.sh | sh
```

The prebuilt binary supports **Linux x86_64 with glibc 2.39 or newer** (for example, Ubuntu 24.04). It needs an X11 desktop and PulseAudio or PipeWire's PulseAudio service. On Ubuntu/Debian, install the runtime libraries if needed:

```sh
sudo apt install curl ca-certificates libx11-6 libxext6 libxft2 libxrandr2 libxtst6 libpulse0 libxres1
```

The [installer](install.sh) verifies the release's SHA-256 checksum and checks that the downloaded binary runs before atomically replacing `~/.local/bin/likh`. Login, configuration, and autostart settings are preserved. Ensure `~/.local/bin` is on your PATH. To use another directory, pass `LIKH_INSTALL_DIR=/your/bin` to `sh` in the command above. Restart any running Likh instance after updating.

For the first run:

```sh
likh doctor
likh login
likh run
```

`doctor` reports the desktop/audio checks and login status; before the first login, an unauthenticated status is expected. Build from source below for other Linux architectures or older glibc versions.

## Build and run

Rust 1.89 or newer and a C compiler are required. On Ubuntu/Debian:

```sh
sudo apt install build-essential pkg-config libx11-dev libxext-dev libxft-dev libxrandr-dev libxtst6 libpulse0 libxres1
cargo build --release --locked
./target/release/likh doctor
./target/release/likh login
./target/release/likh run
```

Login opens Claude's browser consent page. Paste the complete `code#state` result or callback URL into the terminal. Use `login --no-browser` to open the URL yourself. Likh uses its own credentials and never reads or changes Claude Code's login.

Click a text input, hold Ctrl and Win together, and speak once the microphone icon and waveform appear. Either left or right Ctrl and either Win/Super key work, in either press order. Recording starts when both modifiers are held and stops when either is released; release both before the final text is pasted. If both Ctrl keys or both Win keys are held, the corresponding modifier stays active until its last key is released. Ctrl alone and Win alone do not start recording. Key autorepeat does not restart recording, including after Escape cancels a session.

The shortcut uses XKB notifications of held modifiers, which are delivered regardless of focus or keyboard grabs. Caps Lock, Num Lock, and latched modifiers do not activate it. Ctrl and Win continue to reach the desktop normally, so remove any desktop action bound to Ctrl+Win if it interferes. Escape cancels while recording or processing, including while Ctrl+Win is still held.

At startup, Likh prints its version and `Hotkey enabled: Ctrl+Win (XKB modifier events, Ctrl=..., Win=...)`. Restart the rebuilt binary with `./target/release/likh run` after updating. `xmodmap -pm` shows the desktop modifier assignments for comparison with this diagnostic.

Recording ends after two minutes even if you keep holding. There is no always-on microphone or local speech model. The audio server connection remains open while idle, but the microphone stream exists only during a recording. The configured default microphone is selected each time.

To install the binary and optionally start it at desktop login:

```sh
install -Dm755 target/release/likh ~/.local/bin/likh
mkdir -p ~/.config/autostart
cp packaging/likh.desktop ~/.config/autostart/likh.desktop
```

Ensure `~/.local/bin` is on your desktop session's PATH, or change `Exec` in the desktop file to the absolute path of your installed binary. Sign in once before enabling autostart. Run in a terminal initially to see diagnostics. Stop with Ctrl+C, SIGTERM, or `pkill -TERM -x likh`.

## Configuration and commands

Copy [config.example.toml](config.example.toml) to `~/.config/likh/config.toml`. `$XDG_CONFIG_HOME` is respected. English is the default; `language` accepts the service's supported language codes. A `microphone` source name overrides the system default. `pactl list short sources` can list names if PulseAudio utilities are installed.

```sh
likh status       # Local login status; does not contact Claude
likh doctor       # X11/XTEST, audio server, configuration, login status
likh copy-last    # Copy the last available transcript from the running app
likh logout       # Remove only Likh's local credentials
```

`--config-dir PATH` selects another configuration directory. `--runtime-dir PATH` selects another private control-socket directory; the default is `$XDG_RUNTIME_DIR/likh`. Commands that contact the daemon must use the same runtime directory. `CLAUDE_CODE_OAUTH_TOKEN`, when set, takes precedence over stored credentials and is not refreshed automatically.

The widget follows the X11 desktop's scale setting: at 200%, the listening pill is 312×88 pixels, including its icon, animation, and rounded corners. Status text scales too. Settings are read when the overlay is shown. To choose a size yourself, run `LIKH_INDICATOR_SCALE=2.25 ./target/release/likh run`. The override accepts factors from 1 to 4 and replaces automatic scaling. Detection uses `GDK_SCALE`, then XSETTINGS `Gdk/WindowScalingFactor`, then `Xft/DPI` or the `Xft.dpi` resource. These are desktop-wide X11 settings; separate scales on mixed-DPI monitors are not inferred.

## Pasting and recovery

Each automatic paste includes a trailing space so consecutive dictations stay separated.

Final text is pasted using the clipboard and Ctrl+V. Common terminals use Ctrl+Shift+V. **Zed uses Shift+Insert**, which invokes text paste in both its editor and integrated terminal, avoiding Codex CLI's Ctrl+V image-paste binding. This follows [Zed's default Linux keymap](https://github.com/zed-industries/zed/blob/main/assets/keymaps/default-linux.json). `[paste_overrides]` can select a different shortcut by lowercase WM_CLASS. Likh offers `UTF8_STRING`, `TEXT`, and the common `text/plain` UTF-8 MIME formats. Latin-1 `STRING` is offered only when the text is representable without loss. Use `xprop WM_CLASS` to inspect an application. Other editors' embedded terminals share their application's class, so they may need a manual override.

If a terminal app reports “clipboard contents were not available in the requested format” or “Failed to paste image,” check the `Paste target: WM_CLASS=..., shortcut=...` diagnostic in Likh's terminal. A raw Ctrl+V can trigger an image-paste command instead of text paste, as reported in [Codex CLI](https://github.com/openai/codex/issues/17103). Set the host application's `[paste_overrides]` entry to `"ctrl-shift-v"` if that is its text-paste shortcut, then restart Likh. An override also affects ordinary inputs in the same host. Diagnostics do not include transcript text or window titles.

Likh saves the previous UTF-8 clipboard text and restores it after the destination requests the transcript, provided clipboard ownership has not changed. XRes, when available, identifies requests from hidden toolkit windows. Restoration is best effort: rich text, images, large/incremental clipboard payloads, and clipboard-manager history are not preserved. If consumption cannot be identified, the transcript remains on the clipboard rather than restoring too early.

If focus changes while recording or processing, Likh keeps the result in memory and skips automatic paste. Run `likh copy-last`, then paste where you want. X11 can track windows, but it cannot reliably distinguish individual fields inside every browser or editor. Keep the intended input focused until insertion completes. Applications that reject simulated paste, nonstandard paste bindings, and password fields are not guaranteed to work.

Network/authentication failures, microphone loss, buffer overflow, and finalization timeout stop the session. Audio is never automatically replayed. Unconfirmed text is available through `copy-last` when received; it is never automatically inserted. Explicit cancellation discards that session. A previous successful result remains available until replaced or the app exits.

## Service and data

This uses the **undocumented Claude Code voice service**, not Anthropic's public Messages API. A Claude account with voice access is required; an Anthropic API key is not interchangeable. Upstream compatibility can change. The protocol and OAuth flow were independently implemented from the local `claude-transcribe` reference project. See [Claude's voice documentation](https://code.claude.com/docs/en/voice-dictation) for account requirements.

Microphone audio is sent to Anthropic while recording. Likh does not write recordings or transcripts to disk, log their contents, run telemetry, or invoke an LLM to rewrite your words. It keeps the most recent transcript in memory for recovery. Clipboard managers can retain pasted text independently.

OAuth tokens are plaintext files protected by private filesystem permissions: directory `0700`, file `0600`, atomic replacement and a refresh lock. Logout removes local credentials; it does not revoke the remote authorization or unset an environment token. Network calls use TLS, bounded timeouts, and no redirect following. Custom voice endpoints are restricted to Anthropic's known voice hosts. HTTP proxy environment variables are not currently supported.

## Development and validation

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./scripts/test-hotkey.sh
./scripts/test-clipboard.sh
./scripts/test-shutdown.sh
./scripts/test-scale.sh
sudo apt install xvfb xauth
./scripts/test-desktop.sh
```

Rust tests use an in-memory WebSocket server, including the HTTP upgrade, so no account, microphone, or listening socket is needed. Native hotkey, clipboard, and shutdown tests also run without a display. The hotkey regression checks XKB subscription, Ctrl+Win press/release orders, repeats, cancellation, and paste injection. The shutdown regression uses a real dynamically loaded test library that registers a display-close callback, checking that X11 cleanup happens before the library is unloaded, including missing/unsupported XRes cases. The separate native integration harness uses an isolated Xvfb display to check modifier notifications during keyboard grabs, both sides of Ctrl/Win, release orders, lock keys, cancellation, focus, Unicode clipboard transfer, and clipboard ownership. Never run that harness directly on your regular display.

The listening indicator is 156×44 logical pixels and uses native X11 drawing, a small double buffer, and a microphone level shared through an atomic value. Shapes are drawn at the display's resolution and Xft renders scaled status text. Animation is capped at about 30 frames per second and stops when hidden; no GUI framework or animation thread is added. X Shape provides rounded window corners when available. Scale parsing tests cover both byte orders, malformed settings, and precedence; the isolated Xvfb harness checks automatic 200% and an explicit fractional scale.

The app reports microphone-start and release-to-paste timings on stderr without text/audio contents. The target is under 100 ms local microphone startup and under 1% idle CPU; server/network latency is separate and not guaranteed. Measure on the actual desktop with `ps -C likh -o pid,rss,pcpu,etime` or `pidstat -p <pid> 1`.

Before relying on daily use, run a live login and microphone test, then dictate into a browser, VS Code, a GTK editor, and GNOME Terminal. Verify holding and releasing Ctrl+Win, Escape, focus changes, and clipboard restoration. Live desktop/audio/service testing was unavailable in the development sandbox because it denies socket access; those scenarios are not claimed as verified.

Implementation: Rust handles OAuth, configuration, audio, WebSocket protocol, and lifecycle. `native/desktop.c` bridges Xlib/XTEST/RandR; `src/audio.rs` binds the small libpulse ABI used by the dedicated audio thread. Hyper/Rustls and tokio-websockets avoid extra runtime processes. The binary links the system X11 and PulseAudio libraries; it is not fully static.
