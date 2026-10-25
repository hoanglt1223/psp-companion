# PSP Companion

Turns a PSP-3000 (ARK-4 CFW) into a low-bandwidth Wi-Fi companion for a PC. Two programs talk over the LAN:

- `host/`: **companion-host** (Rust), runs on the PC as a tray app. It owns everything heavy and everything secret.
- `psp/`: **companion-psp** (C, pspsdk), runs on the PSP. Wi-Fi, discovery, pairing, status screen.

The wire protocol is specified in [`protocol/PROTOCOL.md`](protocol/PROTOCOL.md). Current state: protocol, pairing,
encrypted channels, the secondary screen (widgets, cursor, macro pad), the PC speaker, the Claude voice assistant,
and Stream Deck mode (Bitfocus Companion Satellite surface).

## Wi-Fi budget

The PSP-3000 only has 802.11b. Traffic is push-on-change only, the PSP sends a keepalive at most every 15 seconds,
and the host's token-bucket rate limiter keeps all traffic (both directions) under 300 kbps. The PSP status
screen shows live in/out throughput against that budget.

## Layout

| Path | What |
|------|------|
| `protocol/PROTOCOL.md` | Message spec (discovery, SPAKE2 pairing, framing, encryption, message types) |
| `host/src/net/` | Codec, handshake, messages, ChaCha20-Poly1305 channels, rate limiter, discovery, server |
| `host/src/pairing.rs` | PIN generation and SPAKE2 (both roles) |
| `host/src/config.rs`, `state.rs`, `tray.rs` | Config file, shared state, tray UI |
| `host/tests/e2e.rs` | Drives the real server over localhost as a PSP would |
| `psp/` | PSP client: `main.c` (state machine and screens), `net.c`, `wifi.c`, `ui.c`, `crypto.c`, `protocol.c`, `rng.c`, `storage.c` |
| `host/src/streamdeck/` | Bitfocus Companion Satellite client and the PSP key bridge |
| `host/src/voice/` | ADPCM, mic cleanup, whisper.cpp / hosted STT, Claude streaming client, Piper / hosted TTS, sessions |
| `host/src/audio/` | Loopback capture, MP3 encoding, stream manager (one stream per PSP) |
| `host/src/widgets/`, `macros.rs`, `actions/`, `imaging.rs`, `app.rs` | Widget hub and sources, macro slots, desktop actions, IDX8-LZ4 images, feature router |
| `tools/xcheck/` | Compiles the portable PSP C (`crypto.c`, `protocol.c`, `font.c`, `image.c`, `jitter.c`, `adpcm.c`) for the PC and checks it against the Rust host |
| `tools/fontgen/` | Builds `psp/font_vi.c` from the Unifont subset |

## Host

Requires Rust 1.88+ (edition 2024). On Linux the tray needs `libgtk-3-dev libxdo-dev libayatana-appindicator3-dev`
and the PC speaker `libasound2-dev`; `cargo build --no-default-features` builds without the tray.

```sh
cargo build --release            # host only
cargo test                       # host unit and end-to-end tests
cargo test --workspace           # also the C-vs-Rust cross-checks (needs a C compiler: MSVC, gcc or clang)
```

Run:

```sh
companion-host                   # tray icon: status, live throughput, "Pair new PSP", logs, quit
companion-host --headless --pair # no tray; logs the pairing PIN; stop with Ctrl+C
companion-host --config path/to/config.toml
```

Config and logs live in the per-user directories (on Windows `%APPDATA%\psp-companion\config\config.toml` and
`%LOCALAPPDATA%\psp-companion\data\logs\`). The config is created on first run:

```toml
host_name = "DESKTOP"
host_id = "..."            # generated once

[network]
bind = "0.0.0.0"
discovery_port = 47860     # UDP
control_port = 47861       # TCP
audio_port = 47862         # UDP
budget_kbps = 300          # 1-300; 300 is the hard cap
keepalive_secs = 15        # minimum 15

[[devices]]                # added by pairing; the key is a secret
id = "..."
name = "PSP"
key = "..."
paired_at = 1792745000
```

Windows Firewall must allow inbound UDP 47860 and 47862 and TCP 47861 on the private network, for example:

```powershell
New-NetFirewallRule -DisplayName "PSP Companion" -Direction Inbound -Profile Private -Program "C:\path\to\companion-host.exe" -Action Allow
```

On Linux the tray needs GTK 3 and libappindicator development packages; `--headless` avoids the tray.

## PSP

Built with the pspdev toolchain in Docker, so it works from Windows:

```sh
./scripts/build-psp.sh           # or: pwsh scripts/build-psp.ps1
```

Both run `make` in `psp/` inside `pspdev/pspdev:latest` (override with `PSPDEV_IMAGE`). Copy `psp/EBOOT.PBP` to
`ms0:/PSP/GAME/PSPCompanion/EBOOT.PBP`. The pairing key and settings live in a system save
(`ms0:/PSP/SAVEDATA/PSPCOMPAN0000`); a `pairing.dat` left next to the EBOOT by older versions is moved into it.
`tools/make-psp-art.py` draws `ICON0.PNG` and `PIC1.PNG`.

Smoke test without a PSP (`ppsspp-headless:local` image; PPSSPP needs a larger `/dev/shm` than Docker's 64 MB):

```sh
mkdir -p ms/PSP/GAME/PSPCompanion && cp psp/EBOOT.PBP ms/PSP/GAME/PSPCompanion/
docker run --rm --shm-size=1g -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD:/work" -w /work ppsspp-headless:local \
  PPSSPPHeadless --memstick=/work/ms --timeout-emulated=20 --screenshot-at=20:pairing.png /work/ms/PSP/GAME/PSPCompanion/EBOOT.PBP
```

## Packaging and releases

User guides: [docs/user/INSTALL.md](docs/user/INSTALL.md) (install, pairing, settings),
[docs/user/FEATURES.md](docs/user/FEATURES.md), [docs/user/TROUBLESHOOTING.md](docs/user/TROUBLESHOOTING.md).

- Windows installer: `host/packaging/windows/companion-host.iss` (Inno Setup 6; autostart and firewall rule as
  options): `iscc /DAppVersion=0.2.0 host\packaging\windows\companion-host.iss` after `cargo build --release`.
- Linux: `sh host/packaging/linux/package.sh [version]` builds `out/packages/companion-host-<version>-linux-x86_64.tar.gz`
  (headless build, `install.sh`, systemd user unit `companion-host.service`).
- `.github/workflows/ci.yml` runs on pushes and pull requests: Linux tests, clippy and rustfmt, the headless build,
  and the EBOOT build.
- Pushing a `v*` tag runs `.github/workflows/release.yml`: it tests on Windows and Linux and attaches the Windows
  installer and exe, the Linux tarball, and `psp-companion-<tag>-psp.zip` (unzip onto the Memory Stick root) to a
  GitHub Release.

On the PSP:

1. Pick the Wi-Fi profile in the system dialog.
2. On the PC, choose **Pair new PSP** in the tray (or run with `--pair`). A 6-digit PIN appears.
3. On the PSP, select the PC from the list (or press Triangle to type its IP) and enter the PIN.
4. The widget screen appears; the status screen (from the START menu) shows the PC, RTT, live throughput,
   totals and the audio channel state.
   START opens the menu; on the status screen Square measures RTT now and SELECT twice forgets the pairing.

### Testing on a PSP-3000 (ARK-4)

1. Build (`./scripts/build-psp.sh`) and copy `psp/EBOOT.PBP` to `ms0:/PSP/GAME/PSPCompanion/` over USB.
2. On the PC, start the host (`cargo run -p companion-host --release`) and allow it through the Windows firewall
   (private network). Use a 2.4 GHz network the PSP can join (see *Router settings for 802.11b*).
3. On the PSP: Game > Memory Stick > PSP Companion. Pair as above.
4. Check, in order: widgets update for an hour; music plays 30 minutes without dropouts; a voice question gets an
   answer within about 3 s; with `[streamdeck] enabled = true` and Companion running, keys press OBS/Spotify actions.
5. For crash logs, run via psplink instead: `usbhostfs_pc` + `pspsh`, then run the ELF (`./companion.elf`) from `host0:` in the
   `psp/` folder; exceptions print in `pspsh`.
6. Report what fails, with the status-screen numbers (RTT, throughput).

When Wi-Fi drops, the PSP reconnects to the same profile; when the PC disappears, it backs off
(1, 2, 4 ... 30 s) and, after repeated failures, rediscovers the PC by its host ID in case its IP changed.

## Secondary screen: widgets and macros

After connecting, the PSP shows the widget screen. **L/R** switch pages, the **analog nub** moves a cursor and
**X** taps a widget, **Triangle** opens the 12-slot macro pad, **START** opens the menu (widgets, connection status,
Wi-Fi). The host pushes a widget only when its value changes, at most once per second; the clock runs on the PSP.

The layout is `layout.toml` next to `config.toml` (created on first run): up to three pages and 16 widgets of kind
`clock`, `system`, `now_playing`, `notifications`, `calendar`, `claude_code`, each with a position on the
480x272 screen and an optional `tap` action. Macro slots and widget sources live in `config.toml`:

```toml
[widgets]
system_interval_secs = 2     # CPU, GPU (Windows performance counters), RAM, temperatures when exposed
now_playing = true           # Windows media session: title, artist, 64x64 cover sent once per track
calendar_ics = ""            # .ics file path or a calendar's private iCal URL; recurring events supported
calendar_refresh_mins = 10
ingest_port = 47863          # local endpoint for notifications and Claude Code (127.0.0.1 only; 0 = off)
ingest_token = ""            # if set, required in the X-Companion-Token header

[[macros]]                   # slots 0-11; the PSP only ever sends the slot number
slot = 0
label = "Play/Pause"
action = { type = "media", key = "play_pause" }   # play_pause, next, previous, stop, volume_up/down, mute
# other actions: { type = "mute_mic" }, { type = "hotkey", keys = "ctrl+shift+m" },
#   { type = "command", program = "C:\\tools\\x.exe", args = ["--flag"] }, { type = "open", target = "https://..." }
```

Notifications from any script:

```sh
curl -X POST http://127.0.0.1:47863/notify -H "Content-Type: application/json" -d "{\"title\":\"Build passed\",\"body\":\"main is green\",\"app\":\"CI\"}"
curl -X DELETE http://127.0.0.1:47863/notify
```

Claude Code session status: add a hook for the events `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
`Notification`, `Stop`, and `SessionEnd` in `~/.claude/settings.json` that pipes the hook input to the host:

```json
{ "type": "command", "command": "curl -s -X POST http://127.0.0.1:47863/claude-code -H \"Content-Type: application/json\" --data-binary @-" }
```

The PSP font is a subset of GNU Unifont 18.0.01 (SIL Open Font License 1.1, `psp/assets/OFL-1.1.txt`) with every
Vietnamese letter; `cargo run -p pspc-fontgen` regenerates `psp/font_vi.c` from `psp/assets/font-vi.hex`.

## Voice assistant

START menu, **Assistant**. Hold **R** and speak (Vietnamese or English), release to send. The transcript and Claude's
reply appear as text and the reply is spoken through the PSP speakers, starting with the first sentence while the
rest is still being written. **L+R** stops the spoken reply; UP/DOWN scroll the conversation. The microphone audio
(11 kHz IMA ADPCM, about 44 kbps) is sent only while R is held, for at most 60 seconds. The PC keeps the
conversation history (last 10 exchanges per PSP) and owns the API key; the PSP never talks to the internet.

The host needs a Claude API key and a speech-to-text engine; text-to-speech is optional (text-only replies without it).
whisper.cpp and Piper are installed by you and configured by path; nothing large is downloaded by this project.

```toml
[voice]
model = "claude-opus-5-5"        # "claude-haiku-5-5" answers faster, for lower voice latency
api_key = ""                     # empty: uses the ANTHROPIC_API_KEY environment variable (recommended)
max_tokens = 400
history_turns = 10
save_wav_dir = ""                # set to a folder to keep each utterance as a WAV (checking the PSP mic)

[voice.stt]
engine = "whisper_cpp"           # or "openai" for any OpenAI-compatible /v1/audio/transcriptions service
whisper_bin = 'C:\tools\whisper\whisper-cli.exe'
whisper_model = 'C:\tools\whisper\ggml-small.bin'   # small or larger for good Vietnamese
# url = "https://api.openai.com/v1/audio/transcriptions"; model = "whisper-1"; api_key_env = "OPENAI_API_KEY"

[voice.tts]
engine = "piper"                 # or "openai" (any /v1/audio/speech service), or "none"
piper_bin = 'C:\tools\piper\piper.exe'
piper_voice_vi = 'C:\tools\piper\vi_VN-vais1000-medium.onnx'   # .onnx.json must sit next to it
piper_voice_en = 'C:\tools\piper\en_US-amy-medium.onnx'
```

The host cleans the microphone signal (high-pass, noise gate, normalisation) before speech-to-text, and reports
problems on the PSP in plain words (no key, rate limited, overloaded, no speech heard). Tests that need whisper.cpp
or Piper run only when `PSPC_WHISPER_BIN`/`PSPC_WHISPER_MODEL` or `PSPC_PIPER_BIN`/`PSPC_PIPER_VOICE` are set.

## Stream Deck mode (Bitfocus Companion)

With [Bitfocus Companion](https://bitfocus.io/companion) running on the PC (Satellite API 1.5 to 1.x, Companion 3.2
or later; page switching needs 4.3 or later), each connected PSP appears in Companion's Surfaces list as a 6x3
"PSP Companion" surface. START menu, **Stream Deck**: D-pad or the nub selects, X presses (held keys stay held, so
long presses work), L/R switch Companion pages, Triangle opens the built-in macro pad. Any Companion integration
works (OBS scene switching, Spotify, vMix, Home Assistant, hotkeys). The client was written against Satellite API
1.15 (Companion 5.2, `TESTED_API` in `host/src/streamdeck/satellite.rs`); newer 1.x servers connect with a log notice.

Key images are 76x76, palettised and compressed (a flat button is a few hundred bytes), cached on the PSP by ID, and
only changed keys are sent: at most 5 new images per second, while keys the PSP already has (flipping back to a
page) cost a few bytes each. Animated keys are shown as a still frame. When Companion is not running, the screen says
so and the macro pad remains available.

```toml
[streamdeck]
enabled = false  # set true when Bitfocus Companion runs
host = "127.0.0.1"   # the PC running Companion
port = 16622         # Companion's Satellite TCP port
bitmap_size = 76     # 32-96
```

## PC speaker

START menu, **PC speaker**, X turns it on or off; UP/DOWN set the app volume (the PSP's volume buttons work too).
The host captures what the PC plays (WASAPI loopback, nothing is played or recorded from a microphone), encodes MP3
at 96 kbps stereo (about 110 kbps on the air), and sends one frame per UDP packet; silent stretches send nothing.
The PSP decodes on the Media Engine behind an adaptive 150-250 ms jitter buffer and conceals lost frames. A voice
assistant reply pauses the speaker and it resumes afterwards (one audio stream at a time).

```toml
[audio]
speaker_device = ""   # output device to capture; empty = the Windows default output
```

To keep the PC itself silent while the PSP plays, make an unused or virtual output device (for example a virtual
audio cable) the Windows default output; the default-device capture then follows it. This is a manual, reversible
Windows setting; the host never changes it.

### Router settings for 802.11b

Also in [docs/user/TROUBLESHOOTING.md](docs/user/TROUBLESHOOTING.md#router-settings-for-80211b).

- A **2.4 GHz** SSID with 802.11b allowed (mode "b/g/n mixed", not "g/n only" or "n only").
- **WPA2-Personal with AES (CCMP)**. The PSP cannot join WPA3, and WPA2/WPA3 transition mode often fails
  (management-frame protection). Use a separate 2.4 GHz guest or IoT SSID with plain WPA2-AES if needed.
- Turn off **client/AP isolation** on that SSID, or the PSP cannot reach the PC.
- Channel width 20 MHz and a fixed channel (1, 6 or 11) are the most reliable.
- The PC and the PSP must be on the same subnet for broadcast discovery; otherwise use manual IP entry.

## Security notes

- The PIN is never sent. Pairing is SPAKE2 (RFC 9382) over edwards25519, so a passive listener learns nothing,
  and an active attacker gets one guess per attempt; each PIN allows 3 attempts within 2 minutes.
- Every session derives fresh keys from the pairing key and both sides' nonces; all control and audio traffic is
  ChaCha20-Poly1305 with per-direction keys and counters.
- The PSP has no user-mode CSPRNG. `psp/rng.c` seeds from the microphone noise floor (when available), timer
  jitter and clocks, then keeps mixing in input and packet timings. The host's nonce guarantees fresh session keys
  even if the PSP's randomness is weak; the PSP's randomness matters most for the one-time pairing.
- Pairing keys are stored in plain form in the host config (per-user directory) and on the Memory Stick.

## License

MIT. See [LICENSE](LICENSE). The host statically links LAME (LGPL-2.0-or-later) through `mp3lame-sys` for MP3 encoding;
[THIRD_PARTY.md](THIRD_PARTY.md) has the notice and how to relink with a modified LAME (releases ship it next to the exe). The PSP font is
a GNU Unifont subset under the SIL Open Font License 1.1.
