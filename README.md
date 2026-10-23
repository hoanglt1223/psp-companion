# PSP Companion

Turns a PSP-3000 (ARK-4 CFW) into a low-bandwidth Wi-Fi companion for a PC. Two programs talk over the LAN:

- `host/`: **companion-host** (Rust), runs on the PC as a tray app. It owns everything heavy and everything secret.
- `psp/`: **companion-psp** (C, pspsdk), runs on the PSP. Wi-Fi, discovery, pairing, status screen.

The wire protocol is specified in [`protocol/PROTOCOL.md`](protocol/PROTOCOL.md). Current state: protocol, pairing,
encrypted channels, the secondary screen (widgets, cursor, macro pad), and the PC speaker. Voice assistant and Stream
Deck mode follow.

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
| `host/src/audio/` | Loopback capture, MP3 encoding, stream manager (one stream per PSP) |
| `host/src/widgets/`, `macros.rs`, `actions/`, `imaging.rs`, `app.rs` | Widget hub and sources, macro slots, desktop actions, IDX8-LZ4 images, feature router |
| `tools/xcheck/` | Compiles the portable PSP C (`crypto.c`, `protocol.c`, `font.c`, `image.c`, `jitter.c`) for the PC and checks it against the Rust host |
| `tools/fontgen/` | Builds `psp/font_vi.c` from the Unifont subset |

## Host

Requires Rust 1.85+ (edition 2024).

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
`ms0:/PSP/GAME/PSPCompanion/EBOOT.PBP`. The pairing record is saved as `pairing.dat` next to the EBOOT.

On the PSP:

1. Pick the Wi-Fi profile in the system dialog.
2. On the PC, choose **Pair new PSP** in the tray (or run with `--pair`). A 6-digit PIN appears.
3. On the PSP, select the PC from the list (or press Triangle to type its IP) and enter the PIN.
4. The widget screen appears; the status screen (from the START menu) shows the PC, RTT, live throughput,
   totals and the audio channel state.
   START opens the menu; on the status screen Square measures RTT now and SELECT twice forgets the pairing.

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

MIT. See [LICENSE](LICENSE). The host links LAME (LGPL-2.0-or-later) through `mp3lame-sys` for MP3 encoding;
since this whole program is open source, rebuilding it from source satisfies the LGPL relinking terms. The PSP font is
a GNU Unifont subset under the SIL Open Font License 1.1.
