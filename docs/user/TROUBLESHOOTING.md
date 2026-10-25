# Troubleshooting

## Router settings for 802.11b

The PSP-3000 is an 802.11b device from 2008. Most join problems are router settings:

- A **2.4 GHz** SSID with 802.11b allowed (mode "b/g/n mixed", not "g/n only" or "n only").
- **WPA2-Personal with AES (CCMP)**. The PSP cannot join WPA3, and WPA2/WPA3 transition mode often fails
  (management-frame protection). Use a separate 2.4 GHz guest or IoT SSID with plain WPA2-AES if needed.
- Turn off **client/AP isolation** on that SSID, or the PSP cannot reach the PC.
- Channel width 20 MHz and a fixed channel (1, 6 or 11) are the most reliable.
- The PC and the PSP must be on the same subnet for discovery; otherwise type the PC's IP on the PSP (Triangle on
  the pairing screen).

Test the profile in XMB > Settings > Network Settings first: if the system's connection test fails, the app cannot
connect either.

## "No PC found"

- Is companion-host running? (Windows: the tray icon; Linux: `systemctl --user status companion-host`.)
- Firewall: inbound UDP 47860 (discovery) and 47862 (audio) and TCP 47861 (control) must be allowed. On Windows the
  network must be **Private**; the installer's rule only covers private networks. On Linux with ufw:
  `sudo ufw allow 47860:47862/udp && sudo ufw allow 47861/tcp`.
- VPN software on the PC often blocks broadcast discovery. Press Triangle on the PSP and type the PC's IP address.
- Guest networks and mesh systems may isolate clients; see the router settings above.

## Pairing fails

- "Wrong PIN." or "Too many wrong PINs": a PIN allows 3 attempts in 2 minutes. Open pairing again on the PC for a
  new PIN.
- "Pairing is not open": open pairing on the PC first (the PIN also expires after 2 minutes), then try again.
- After reinstalling the host or deleting its config, the PC no longer knows the PSP: on the PSP, Menu > Settings >
  Forget this PC, then pair again.

## Connection drops or is slow

The status page (Menu > Settings > Connection status) shows the round-trip time and traffic. On 802.11b a
round-trip time under 30 ms and a few kbps for widgets are normal; music adds about 110 kbps.

- Move the PSP closer to the router or use a less busy channel (1, 6 or 11).
- WLAN power save adds latency; turn it off (Menu > Settings) if the PC speaker stutters.
- The PSP reconnects on its own after a drop and backs off up to 30 seconds while the PC is unreachable.

## Voice assistant

The PSP shows the reason in plain words:

- "No Claude API key": set `ANTHROPIC_API_KEY` or `voice.api_key` ([INSTALL.md](INSTALL.md#5-settings)), then
  restart the host.
- "I did not hear anything" or "I could not make out any words": hold R a moment before speaking. To check the
  microphone, set *Save each utterance as WAV in* on the settings page and listen to the files.
- "Speech recognition failed on the PC" or "Speech recognition: ...": check the whisper.cpp program and model paths
  on the settings page, and the host log.
- Slow answers: choose `claude-haiku-5-5`, or a smaller whisper.cpp model (`base` is fast, `small` is better for
  Vietnamese).

## Stream Deck mode says Companion is not running

Start Bitfocus Companion, check that the host and port on the settings page match Companion's Satellite settings
(default 127.0.0.1:16622), and that Stream Deck mode is turned on. Companion older than 3.2 is not supported.

## The settings page does not open

It is served on 127.0.0.1 at the port in `widgets.ingest_port` (default 47863). If another program uses that
port, change it in `config.toml` and restart the host. Setting the port to 0 turns the page off.

## Logs

- Windows: tray menu > Open log folder (`%LOCALAPPDATA%\psp-companion\data\logs\`).
- Linux: `journalctl --user -u companion-host`, and `~/.local/share/psp-companion/logs/`.
- PSP crashes: run the app through psplink (see the project README) and include the output.

When reporting a problem, include the log and the numbers from the PSP's connection status page.
