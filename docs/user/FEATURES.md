# Features and controls

Press **START** on the PSP at any time for the menu: Screen (widgets), Assistant, Macros, Stream Deck, PC speaker,
Settings. **HOME** quits. The top bar always shows the Wi-Fi signal and the battery.

The PSP-3000 has only 802.11b Wi-Fi, so everything is designed to stay small on the network: the PC pushes data
only when something changes, images are cached on the PSP, and only one audio stream plays at a time. The
connection status page shows the live traffic.

## Widget screen

Clock, CPU/GPU/RAM, now playing (with cover art), notifications, the next calendar event, and Claude Code session
status. **L/R** switch pages, the **analog nub** moves a cursor and **X** taps a widget (for example play/pause on
now playing), **Triangle** opens the macro pad.

Configure in the settings page (Widgets): system refresh interval, now playing on/off, a calendar (`.ics` file or a
private iCal URL), and the layout file. Any script can post a notification to the PC host:

```sh
curl -X POST http://127.0.0.1:47863/notify -H "Content-Type: application/json" \
     -d '{"title":"Build passed","body":"main is green","app":"CI"}'
```

The Claude Code hook setup is in the project README.

## Macro pad

Twelve buttons, each mapped to an action on the PC: media keys, mute the microphone, a keyboard shortcut, run a
program, or open a file or web page. Set them in the settings page (Macros). The PSP sends only the slot number;
the PC decides what it does.

## Voice assistant (Claude)

Menu > **Assistant**. Hold **R**, speak in Vietnamese or English, release. The question and Claude's answer appear
as text and the answer is spoken through the PSP speakers, starting with the first sentence. **L+R** stops the
voice; UP/DOWN scroll. Recording stops after 60 seconds. The PC keeps the last 10 exchanges per PSP.

The PC needs:

- A Claude API key (see [INSTALL.md](INSTALL.md#5-settings)).
- Speech to text: [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (free, runs on the PC; use the `small`
  model or larger for good Vietnamese) or any OpenAI-compatible transcription service.
- Text to speech (optional; without it replies are text only): [Piper](https://github.com/rhasspy/piper) with a
  Vietnamese voice (for example `vi_VN-vais1000-medium`) and an English voice (for example `en_US-amy-medium`), or
  any OpenAI-compatible speech service. Keep each voice's `.onnx.json` next to its `.onnx` file.

Point the settings page (Speech to text, Text to speech) at the programs and model files. In the Claude section,
`claude-haiku-5-5` answers faster than the default `claude-opus-5-5`.

## Stream Deck mode (Bitfocus Companion)

Menu > **Stream Deck**. With [Bitfocus Companion](https://bitfocus.io/companion) running on the PC, the PSP becomes
a 6x3 button surface showing live key images: **D-pad** or the nub selects, **X** presses (holding works), **L/R**
change Companion pages, **Triangle** opens the macro pad. Any Companion integration works (OBS, vMix, Spotify,
Home Assistant, hotkeys).

Turn it on in the settings page (Stream Deck: *Connect to Companion*), then restart the host. The PSP appears in
Companion under Surfaces as "PSP Companion".

Supported Companion versions: Satellite API 1.5 to 1.x, which is Companion 3.2 or later; changing pages from the PSP
needs Companion 4.3 or later. The host was written against Satellite API 1.15 (Companion 5.2); newer 1.x versions
connect and log a notice. A future Satellite API 2.x is refused (the host log says why) until the host supports it.

## PC speaker

Menu > **PC speaker**. **X** turns it on or off; UP/DOWN (or the PSP volume buttons) change the volume. What the PC
plays comes out of the PSP with about 150-250 ms of delay, as 96 kbps MP3. Nothing is recorded from a microphone.
A voice assistant answer pauses the speaker; it resumes afterwards.

- Windows: captures the default output (or the device named in Settings > PC speaker). To keep the PC itself quiet,
  make an unused or virtual output device the Windows default.
- Linux: set the capture device to `pulse` (PulseAudio or PipeWire); the systemd service already sets
  `PULSE_SOURCE=@DEFAULT_MONITOR@`, which is "what the PC plays". Running by hand:
  `PULSE_SOURCE=@DEFAULT_MONITOR@ companion-host --headless`.

## Battery and settings on the PSP

Menu > **Settings**:

- **Connection status**: the PC, round-trip time, live in/out traffic, totals and the audio state. Square measures
  the round-trip time now; SELECT twice forgets the pairing.
- **Keep the screen on**: off by default, so the screen dims and turns off after the time set in XMB > Settings >
  Power Save Settings. Any button wakes it. The PSP never goes to sleep by itself while the app runs.
- **Lower CPU clock when idle**: on by default; after 15 seconds without input or audio the CPU runs at 133 MHz
  instead of 222 MHz.
- **WLAN power save**: the system setting; saves battery between updates and applies on the next Wi-Fi connection.
- **Choose Wi-Fi network**, **Forget this PC**.

The battery level and estimated time left are shown on the Settings page. When the PSP wakes from sleep, the app
rejoins Wi-Fi and reconnects to the PC on its own.
