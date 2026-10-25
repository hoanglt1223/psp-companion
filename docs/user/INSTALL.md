# Install and pair

PSP Companion has two parts: **companion-host** on your PC and **PSP Companion** on a PSP-3000 running custom
firmware (ARK-4). Both come from the project's GitHub Releases page.

## 1. Wi-Fi first

The PSP-3000 only speaks 802.11b on 2.4 GHz with WPA2-AES. Before anything else, make sure the PSP can join your
network (XMB > Settings > Network Settings > Infrastructure Mode). If it cannot, see
[TROUBLESHOOTING.md](TROUBLESHOOTING.md#router-settings-for-80211b). The PC and the PSP must be on the same network.

## 2. Install the PSP app

1. Download `psp-companion-<version>-psp.zip`.
2. Connect the PSP over USB (XMB > Settings > USB Connection) and unzip the file onto the root of the Memory Stick,
   so that the app ends up at `PSP/GAME/PSPCompanion/EBOOT.PBP`.
3. On the PSP: Game > Memory Stick > **PSP Companion**.

ARK-4 runs unsigned homebrew from `PSP/GAME` directly; no plugin is needed. The app keeps its pairing and settings
in a normal save (`PSP/SAVEDATA/PSPCOMPAN0000`, shown as "PSP Companion" under Saved Data). Deleting that save
forgets the pairing. Older versions kept `pairing.dat` next to the EBOOT; it is moved into the save on first start.

## 3. Install the PC host

### Windows 10/11

1. Run `companion-host-v<version>-windows-x64-setup.exe`. Keep both options ticked:
   - *Start PSP Companion when I sign in* adds a per-user autostart entry.
   - *Allow PSP connections through Windows Firewall* adds an inbound rule for private networks.
2. A PSP Companion icon appears in the notification area. Its menu shows the status, live traffic, and
   **Pair new PSP**, **Settings...**, the log and config folders, and Quit.

Your network must be marked **Private** in Windows (Settings > Network & internet > Properties), or the firewall
rule does not apply. A standalone `companion-host-<version>-windows-x64.exe` is also published for people who do not
want an installer; it behaves the same but adds no firewall rule or autostart.

### Linux (x86_64)

```sh
tar xzf companion-host-v<version>-linux-x86_64.tar.gz
cd companion-host-v<version>-linux-x86_64
./install.sh            # ~/.local/bin/companion-host + a systemd user service started now and at login
./install.sh --uninstall
```

The Linux build has no tray icon; use the settings page instead (below). Logs:
`journalctl --user -u companion-host -f`. To keep the service running while you are logged out:
`loginctl enable-linger $USER`. If a firewall is active, allow UDP 47860 and 47862 and TCP 47861, for example
`sudo ufw allow 47860:47862/udp && sudo ufw allow 47861/tcp`.

Without systemd, run `companion-host --headless` from a terminal or your desktop's autostart.

## 4. Pair

1. On the PC, open pairing:
   - Windows tray: **Pair new PSP (show PIN)**.
   - Any system: the settings page at <http://127.0.0.1:47863/settings>, button **Pair new PSP**.
   - Or start the host once with `companion-host --headless --pair`; the PIN is in the log.
2. On the PSP, choose the Wi-Fi profile when asked. The app searches for PCs.
3. Select your PC with the D-pad and press **X** (or press **Triangle** to type the PC's IP address).
4. Enter the 6-digit PIN on the on-screen keyboard. A PIN is valid for 2 minutes and 3 attempts.

After pairing, the PSP reconnects to that PC on its own every time the app starts, even if the PC's IP address
changes. To pair with another PC: START menu > Settings > **Forget this PC**.

The PIN is never sent over the air; see the security notes in the project README.

## 5. Settings

The host's settings live in `config.toml` (Windows: `%APPDATA%\psp-companion\config\`, Linux:
`~/.config/psp-companion/`). The settings page at <http://127.0.0.1:47863/settings> edits the voice assistant
(Claude model, speech-to-text and text-to-speech engines), the PC speaker device, Stream Deck mode, widgets and the
12 macro slots. Changes are saved to `config.toml` and apply when the host restarts. The page only answers on this
PC (127.0.0.1).

The Claude API key is never shown on the page. Set the `ANTHROPIC_API_KEY` environment variable (Windows: System >
About > Advanced system settings > Environment Variables; Linux service: a line `ANTHROPIC_API_KEY=...` in
`~/.config/psp-companion/env`) or put it in `config.toml` as `voice.api_key`.

What each feature does and how to use it on the PSP: [FEATURES.md](FEATURES.md).
