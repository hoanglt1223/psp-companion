# PSP Companion wire protocol, version 1

This document is the contract between `companion-psp` (the PSP-3000 client) and
`companion-host` (the PC). Both implementations must follow it byte for byte.

## Design rules

The PSP-3000 has 802.11b Wi-Fi with about 1-2 Mbps of real throughput, so the
protocol is built around a small, fixed budget:

- **Push on change only.** The host sends data when it changes. The PSP never
  polls for data. The only periodic PSP traffic is the keepalive.
- **Keepalive at most every 15 seconds.** The PSP sends `Ping` no more often
  than the `keepalive_s` value in `HelloAck` (minimum and default 15). A user
  action (a manual RTT test) may send one extra `Ping`, at most once per second.
- **300 kbps total cap.** The host runs a token-bucket rate limiter over all
  traffic in both directions (see [Rate limiting](#rate-limiting)).
- **One audio stream at a time.** Starting a stream stops the previous one.
- **No video.** There are no message types for screen or game video.

## Byte order and primitive types

All integers are **little-endian** (both CPUs are little-endian).

| Name    | Encoding |
|---------|----------|
| `u8`, `u16`, `u32`, `u64` | unsigned little-endian integers |
| `str8`  | `u8` byte length, then that many bytes of UTF-8 |
| `str16` | `u16` byte length, then that many bytes of UTF-8 |
| `rest`  | all remaining bytes of the message |
| `id16`  | 16 raw bytes (device or host identifier) |

A decoder must reject a message whose fields run past its end, whose strings
are not valid UTF-8, or that has trailing bytes after the last field (except
for `rest`).

## Ports

| Port  | Transport | Direction | Purpose |
|-------|-----------|-----------|---------|
| 47860 | UDP | PSP broadcast to host, host unicast reply | Discovery |
| 47861 | TCP | PSP connects to host | Pairing and the control channel |
| 47862 | UDP | Both | Audio channel (host port; the PSP uses an ephemeral port) |

The host announces its actual control and audio ports in the discovery reply and
in `HelloAck`, so the defaults can be changed in the host config.

## Discovery

The PSP broadcasts a request to `255.255.255.255:47860` only while it is
looking for a host (at start-up, after the user asks for a rescan, or while
reconnecting with no known host address). It sends at most one request per
second and stops after five unanswered requests until the next reconnect
attempt. Discovery is not authenticated; it only finds candidate addresses.
Authentication happens on the control channel.

Request (14 bytes):

| Field | Type | Value |
|-------|------|-------|
| magic | 4 bytes | `"PSPC"` |
| version | `u8` | `1` |
| kind | `u8` | `0x01` (discover) |
| nonce | 8 bytes | random, echoed in the reply |

Reply (unicast to the sender's address and port):

| Field | Type | Value |
|-------|------|-------|
| magic | 4 bytes | `"PSPC"` |
| version | `u8` | `1` |
| kind | `u8` | `0x02` (announce) |
| nonce | 8 bytes | copied from the request |
| host_id | `id16` | stable host identifier |
| control_port | `u16` | TCP control port |
| audio_port | `u16` | UDP audio port |
| flags | `u8` | bit 0: pairing is open (a PIN is being shown) |
| host_name | `str8` | at most 32 bytes |

A host ignores requests with a wrong magic, version, or kind. A PSP ignores
replies whose nonce does not match its last request.

## Control channel framing (TCP)

Every message on the TCP connection is a frame:

| Field | Type |
|-------|------|
| length | `u16` length of `body` |
| body | `length` bytes |

`length` must be between 1 and 16400 (`MAX_FRAME_BODY`). A larger or zero
length closes the connection. Before the handshake completes, `body` is a
cleartext handshake message. After it completes, `body` is
`ChaCha20-Poly1305(plaintext) || tag` as described in
[Encrypted frames](#encrypted-frames).

### Handshake messages (cleartext)

Every handshake message starts with:

| Field | Type | Value |
|-------|------|-------|
| magic | 4 bytes | `"PSPC"` |
| version | `u8` | `1` |
| kind | `u8` | see below |

| Kind | Name | Direction | Fields after the header |
|------|------|-----------|-------------------------|
| `0x01` | `SessionHello` | PSP to host | `device_id: id16`, `client_nonce: 16 bytes` |
| `0x02` | `SessionReply` | host to PSP | `status: u8`, `host_id: id16`, `server_nonce: 16 bytes` |
| `0x03` | `PairRequest` | PSP to host | `device_id: id16`, `pA: 32 bytes`, `device_name: str8` |
| `0x04` | `PairChallenge` | host to PSP | `status: u8`, `host_id: id16`, `pB: 32 bytes`, `cB: 32 bytes` |
| `0x05` | `PairConfirm` | PSP to host | `cA: 32 bytes` |
| `0x06` | `PairResult` | host to PSP | `status: u8` |

Status values:

| Value | Meaning |
|-------|---------|
| 0 | OK |
| 1 | Unknown device (session) or pairing not open (pairing) |
| 2 | Unsupported protocol version |
| 3 | Too many failed PIN attempts; the PIN is void |
| 4 | Wrong PIN (key confirmation failed) |
| 5 | Busy (host is shutting down or overloaded) |

When a status is not 0, the remaining fields of the reply are zero-filled and
the host closes the connection after sending it. A version mismatch is answered
with status 2 and the host's own version in the header.

A connection carries either one pairing exchange or one session, never both.
After a successful pairing the PSP closes the connection and opens a new one
with `SessionHello`.

### Pairing (SPAKE2 over edwards25519)

Pairing follows RFC 9382 (SPAKE2) on edwards25519 with these fixed choices.
The host shows a fresh 6-digit PIN; the user types it on the PSP. Each PIN is
valid for 120 seconds and for at most 3 `PairRequest` attempts, then it is void.
A passive listener learns nothing about the PIN, and an active attacker gets one
guess per attempt (one in a million).

- Group: edwards25519, generator `G` (RFC 8032), order `L`, cofactor `h = 8`.
- `M = d048032c6ea0b6d697ddc2e86bda85a33adac920f1bf18e1b0c6d166a5cecdaf`
- `N = d3bfb518f44f3430f29d0c92af503865a1ed3281dc69b35dd868ba85f886c4ab`
  (compressed points from RFC 9382, section 4).
- Password scalar: `w = SHA-512("PSPC-PIN-v1" || pin) mod L`, where `pin` is
  the six ASCII digits. `w` is encoded in the transcript as the 32-byte
  little-endian scalar.
- PSP is party A with identity `A = device_id`; host is party B with
  identity `B = host_id`.
- PSP: random scalar `x` (64 random bytes reduced mod `L`),
  `pA = x*G + w*M`. Host: random `y`, `pB = y*G + w*N`. Both are sent as
  32-byte compressed points.
- PSP: `K = h*x*(pB - w*N)`. Host: `K = h*y*(pA - w*M)`. A received point that
  does not decompress, or a `K` that is the identity, aborts the exchange.
- Transcript, with each length an 8-byte little-endian byte count:
  `TT = len(A)||A || len(B)||B || len(pA)||pA || len(pB)||pB || len(K)||K || len(w)||w`.
  `pA` and `pB` are the bytes exactly as received; `K` is its compressed encoding.
- `Ke || Ka = SHA-512(TT)`, 32 bytes each (RFC 9382 with a 512-bit hash, so
  both halves are 256 bits).
- `KcA || KcB = HKDF-SHA256(salt = none, ikm = Ka, info = "ConfirmationKeys", 64 bytes)`.
- `cA = HMAC-SHA256(KcA, TT)`, `cB = HMAC-SHA256(KcB, TT)`.
- `pairing_key = HKDF-SHA256(salt = none, ikm = Ke, info = "PSPC v1 pairing key", 32 bytes)`.

Flow:

1. PSP sends `PairRequest` with `pA`. The host counts one attempt against the PIN.
2. Host replies `PairChallenge` with `pB` and `cB`.
3. PSP checks `cB` in constant time. On mismatch it shows "wrong PIN" and closes.
4. PSP sends `PairConfirm` with `cA`.
5. Host checks `cA`. On success it stores `{device_id, device_name, pairing_key}`,
   voids the PIN, and replies `PairResult(0)`. On failure it replies `PairResult(4)`.
6. PSP stores `{host_id, device_id, pairing_key, host address}` and reconnects
   with a session.

The host closes a pairing connection that is idle for more than 30 seconds.

### Session handshake

1. PSP sends `SessionHello` with its `device_id` and 16 random bytes.
2. The host looks up the device. Unknown device: `SessionReply(1)` and close.
   Otherwise it replies `SessionReply(0)` with 16 random bytes.
3. Both derive the session keys:
   - `th = SHA-256(SessionHello body || SessionReply body)` (the full frame bodies).
   - `okm = HKDF-SHA256(salt = th, ikm = pairing_key, info = "PSPC v1 session keys", 128 bytes)`.
   - `okm` splits into four 32-byte keys, in order: `tcp_c2s`, `tcp_s2c`,
     `udp_c2s`, `udp_s2c` (`c2s` = PSP to host).
4. The PSP sends the encrypted `Hello` message. The host decrypts it; failure
   means the PSP has the wrong key, and the host closes without a reply.
5. The host replies with the encrypted `HelloAck`. The PSP decrypts it; failure
   means the host is not the paired host, and the PSP closes.

A new session from a device closes that device's older session (`Bye` reason 2).

### Encrypted frames

After the handshake, every frame body is:

`ciphertext || tag(16)` = ChaCha20-Poly1305 (RFC 8439) of the plaintext message.

- Key: `tcp_c2s` for PSP-to-host frames, `tcp_s2c` for host-to-PSP frames.
- Nonce (12 bytes): 4 zero bytes, then a `u64` frame counter. Each direction
  has its own counter, starting at 0 and incremented after every frame. The
  counter is not sent; a lost or reordered frame cannot happen on TCP, so a
  decryption failure always closes the connection.
- AAD: the 2-byte frame length field.

Plaintext message: `type: u8` followed by the payload for that type. The
largest plaintext is 16384 bytes (`MAX_MESSAGE`), so `length` never exceeds
16400. Senders split larger objects (images) into several messages.

## Audio channel (UDP)

Datagram layout:

| Field | Type | Notes |
|-------|------|-------|
| kind | `u8` | `0x01` (data) |
| session_tag | `u32` | from `HelloAck`; selects the session without trial decryption |
| seq | `u64` | per-direction counter, starts at 0 |
| body | rest | ChaCha20-Poly1305 of the plaintext message, with tag |

- Key: `udp_c2s` or `udp_s2c`. Nonce: 4 zero bytes then `seq`. AAD: the 13-byte header.
- Plaintext is the same `type: u8 || payload` format as the control channel,
  but only `AudioRegister` (PSP to host) and `AudioPacket` (host to PSP) are
  allowed on UDP.
- The largest datagram is 1200 bytes, to stay below the Wi-Fi MTU without
  IP fragmentation.
- Receivers keep a 64-entry sliding replay window over `seq` and drop
  duplicates, packets older than the window, and packets that fail to decrypt.
  Drops never close the session.

The PSP learns nothing about the host's UDP address from UDP itself: it uses the
host address of the TCP connection and `audio_port` from `HelloAck`. The host
learns the PSP's UDP address from `AudioRegister`. After `HelloAck` the PSP sends
`AudioRegister` and repeats it every 2 seconds (at most 5 times) until it gets
`AudioRegisterAck` on TCP.

## Message types

Direction: C = PSP (client), S = host (server). A receiver closes the connection
with `Error(1)` when a message of a known type arrives in the wrong direction or
fails to decode. Unknown types are logged and ignored, so later minor additions
do not break older peers.

| Type | Name | Dir | Payload | Phase |
|------|------|-----|---------|-------|
| `0x01` | `Hello` | C→S | `caps: u32`, `sw_version: str8`, `device_name: str8` | 01 |
| `0x02` | `HelloAck` | S→C | `caps: u32`, `host_name: str8`, `session_tag: u32`, `audio_port: u16`, `keepalive_s: u16`, `budget_kbps: u16` | 01 |
| `0x03` | `Ping` | C→S | `id: u32`, `t_us: u64` (sender clock) | 01 |
| `0x04` | `Pong` | S→C | `id: u32`, `t_us: u64` (echoed) | 01 |
| `0x05` | `Bye` | both | `reason: u8` (0 normal, 1 shutting down, 2 replaced by a newer session, 3 unpaired) | 01 |
| `0x06` | `Error` | both | `code: u16` (1 protocol violation, 2 unsupported, 3 rate limited, 4 internal), `text: str16` | 01 |
| `0x10` | `WidgetLayout` | S→C | `format: u8`, `data: rest` | 03 |
| `0x11` | `WidgetData` | S→C | `widget_id: u16`, `format: u8` (0 JSON, 1 binary), `data: rest` | 03 |
| `0x12` | `WidgetAsset` | S→C | `asset_id: u64`, `format: u8`, `data: rest` | 03 |
| `0x20` | `InputEvent` | C→S | `kind: u8` (0 tap, 1 button down, 2 button up), `page: u8`, `widget_id: u16`, `x: u16`, `y: u16`, `buttons: u32` | 03 |
| `0x21` | `MacroTrigger` | C→S | `slot: u8` (0-11) | 03 |
| `0x30` | `VoiceStart` | C→S | `utterance_id: u32`, `codec: u8` (1 IMA ADPCM), `sample_rate: u32`, `channels: u8` | 02 |
| `0x31` | `VoiceChunk` | C→S | `utterance_id: u32`, `seq: u32`, `data: rest` | 02 |
| `0x32` | `VoiceEnd` | C→S | `utterance_id: u32`, `reason: u8` (0 released, 1 cancelled, 2 too long) | 02 |
| `0x33` | `VoiceTranscript` | S→C | `utterance_id: u32`, `is_final: u8`, `text: str16` | 02 |
| `0x34` | `VoiceReplyText` | S→C | `utterance_id: u32`, `is_final: u8`, `text: str16` (appended delta) | 02 |
| `0x40` | `AudioStreamStart` | S→C | `stream_id: u8`, `kind: u8` (0 voice reply, 1 PC speaker), `codec: u8` (1 MP3), `sample_rate: u32`, `channels: u8`, `bitrate_kbps: u16` | 02, 04 |
| `0x41` | `AudioStreamStop` | both | `stream_id: u8`, `reason: u8` (0 ended, 1 user stop, 2 preempted by another stream) | 02, 04 |
| `0x42` | `AudioStreamRequest` | C→S | `kind: u8`, `enable: u8` | 04 |
| `0x43` | `AudioPacket` | S→C (UDP) | `stream_id: u8`, `seq: u32`, `pts_ms: u32`, `data: rest` | 02, 04 |
| `0x44` | `AudioRegister` | C→S (UDP) | empty | 01 |
| `0x45` | `AudioRegisterAck` | S→C | empty | 01 |
| `0x50` | `DeckLayout` | S→C | `columns: u8`, `rows: u8`, `key_w: u8`, `key_h: u8` | 06 |
| `0x51` | `DeckKeyImage` | S→C | `key: u8`, `image_id: u64`, `format: u8`, `width: u8`, `height: u8`, `data: rest` (empty: reuse the cached `image_id`) | 06 |
| `0x52` | `DeckKeyPress` | C→S | `key: u8`, `pressed: u8` | 06 |

`caps` bits and the `format` values of widget and deck images are assigned by
the phase that introduces them; version 1 senders set unassigned bits to 0.


## Widgets (phase 03)

The host sends `WidgetLayout` once after `HelloAck`, then the current value of
every widget it has data for (each preceded by any `WidgetAsset` it references
that this session has not received yet). After that it sends `WidgetData` for
a widget only when its encoded value changed, at most once per second per
widget; a newer value replaces one that is still held back. The clock is drawn
from the PSP's own clock and has no data. All text is UTF-8 in Unicode NFC
(precomposed Vietnamese letters) without control characters.

### `WidgetLayout` format 1

| Field | Type | Notes |
|-------|------|-------|
| page_count | `u8` | 1-3; L/R switch pages |
| page titles | `str8` x page_count | |
| widget_count | `u8` | at most 16 |
| widgets | see below, x widget_count | |
| macro_count | `u8` | at most 12 |
| macros | `slot: u8`, `label: str8`, x macro_count | labels of the Triangle macro pad |

Widget entry: `id: u16`, `kind: u8`, `page: u8`, `x: u16`, `y: u16`, `w: u16`,
`h: u16` (screen pixels, 480x272; the top 20 rows hold the page title),
`flags: u8` (bit 0: tapping it runs a host action), `title: str8`.

Kinds: 1 clock, 2 system, 3 now playing, 4 notifications, 5 calendar, 6 Claude Code.

### `WidgetData` format 1, by kind

| Kind | Payload |
|------|---------|
| 2 system | `cpu_pct: u8`, `gpu_pct: u8` (255: unknown), `ram_used_mb: u32`, `ram_total_mb: u32`, `cpu_temp: u16`, `gpu_temp: u16` (signed tenths of a degree C as two's complement; `0x8000`: unknown) |
| 3 now playing | `state: u8` (0 nothing, 1 playing, 2 paused, 3 stopped), `cover_asset: u64` (0: none), `title: str8`, `artist: str8` |
| 4 notifications | `count: u8` (at most 5, newest first), then per entry `app: str8`, `title: str8`, `body: str8`, `time: str8` (host local "HH:MM") |
| 5 calendar | `has_event: u8`; if 1: `title: str8`, `when: str8` (host formatted: "Now", "in 25 min", "Tomorrow 09:00") |
| 6 Claude Code | `count: u8` (at most 4, most recent first), then per entry `project: str8`, `state: u8` (0 idle, 1 working, 2 waiting for the user, 3 done), `detail: str8` |

### `WidgetAsset` format 1 and image format 1 (`IDX8_LZ4`)

Cover art (64x64) and Stream Deck key images share one image format, chosen
because the PSP can expand it with a few lines of C:

| Field | Type |
|-------|------|
| raw_len | `u32`, at most 6 + 768 + 128*128 |
| block | LZ4 block (no frame header) that expands to exactly `raw_len` bytes |

The expanded bytes are `width: u16`, `height: u16` (1-128 each),
`colors: u16` (1-256), `colors` x RGB888, then `width*height` palette indices,
row by row. An index at or above `colors` makes the image invalid. Asset IDs
are the first 8 bytes of SHA-256 of the encoded payload (never 0), so the PSP
may cache by ID and the host never resends an ID a session already has.

### Input

`InputEvent` kind 0 (tap) carries the page, the tapped widget's `id`, and the
cursor position; the host runs the widget's configured action if it has one.
`MacroTrigger` carries only the slot; the host config maps slots to actions.
Kinds 1 and 2 (button down/up) are reserved; the PSP does not send them.


## Audio streams (phases 02 and 04)

Only one stream plays on a PSP at a time. `kind` 0 is a voice reply, `kind` 1
the PC speaker.

- **Codec 1 (MP3)**: MPEG-1 Layer III, 44 100 Hz, constant bitrate, with the bit
  reservoir disabled (`main_data_begin` is 0 in every frame), so each frame
  decodes on its own and a lost packet costs exactly one frame. The PC speaker
  is stereo at 96 kbps; voice replies are mono at 48 kbps.
- The host sends `AudioStreamStart` on TCP before the stream's first packet.
  A PSP drops packets whose `stream_id` is not the current stream.
- `AudioPacket` carries exactly one MP3 frame (1152 samples, about 26 ms).
  `seq` starts at 0 and increases by 1 per packet of the stream; a missing
  `seq` is a lost packet. `pts_ms` is the frame's start time on the stream
  timeline. The host sends nothing during silence (more than 400 ms below about
  -60 dBFS), so `pts_ms` jumps forward while `seq` stays consecutive.
- The PSP asks for the PC speaker with `AudioStreamRequest { kind: 1, enable }`.
  Requests with kind 0 are ignored (voice streams come from the voice assistant).
- A voice reply preempts the speaker: the host sends
  `AudioStreamStop { speaker, reason: 2 }`, then starts the voice stream. When
  the reply has played out, the host sends `AudioStreamStop { voice, reason: 0 }`
  and, if the PSP still wants the speaker, starts it again with a new `stream_id`.
- Voice audio is sent at most 300 ms ahead of real time; speaker audio is sent as
  it is captured.
- The PSP stops the current stream with `AudioStreamStop { stream_id, reason: 1 }`
  (L+R during a reply, or turning the speaker off). The host does not answer it.
- Recommended PSP playback: an adaptive jitter buffer of 150-250 ms; conceal a
  lost frame by repeating the previous one at reduced volume once, then silence.
  After an underrun, buffer up to the target again before playing.


## Voice assistant (phase 02)

- The PSP records only while R is held and sends `VoiceStart`, then
  `VoiceChunk`s as it records, then `VoiceEnd`. `utterance_id` is chosen by
  the PSP and increases per utterance. `seq` starts at 0 per utterance and
  increases by 1; the host drops an utterance whose chunks arrive out of order.
- `VoiceStart.codec` 1 is IMA ADPCM, mono (`channels` 1), at one of 8000, 11025,
  16000 or 22050 Hz. The PSP uses 11025 Hz (about 44 kbps).
- Each `VoiceChunk.data` is self-contained: `predictor: i16`, `step_index: u8`
  (0-88), `reserved: u8` (0), then 4-bit IMA ADPCM codes, low nibble first, two
  samples per byte. The PSP sends 1024 samples (about 93 ms) per chunk.
- Recordings are capped at 60 seconds: the PSP stops at 60 s and sends
  `VoiceEnd` reason 2; the host ignores audio beyond 60 s regardless.
  `VoiceEnd` reason 1 (cancelled) discards the utterance.
- A new `VoiceStart` cancels any reply still being prepared for that PSP.
- The host answers with one `VoiceTranscript` (`is_final` 1), then
  `VoiceReplyText` messages whose `text` fields are appended in order, sent in
  batches about every 250 ms; the last has `is_final` 1. The spoken reply is an
  audio stream of kind 0 (see [Audio streams](#audio-streams-phases-02-and-04)),
  started with the first complete sentence.
- Problems are reported with `Error` codes 2 (not configured on the PC),
  3 (rate limited or overloaded) or 4 (anything else), with text meant to be
  shown as is. These codes do not close the connection; code 1 does.
- Conversation history (the last 10 exchanges by default) is kept on the host
  per paired PSP, across reconnects, until the host restarts.

## Timers

| Timer | Value | Owner |
|-------|-------|-------|
| Keepalive `Ping` | every `keepalive_s` seconds (15) | PSP |
| `Pong` timeout | 10 s, then the PSP reconnects | PSP |
| Idle timeout | `3 * keepalive_s + 5` seconds (50) without any frame, then close | host |
| Pairing connection idle | 30 s | host |
| PIN lifetime | 120 s, 3 attempts | host |
| Reconnect back-off | 1, 2, 4, 8, 16, 30 s (capped) | PSP |

## Rate limiting

The host owns a single token bucket for all PSP traffic:

- Rate: `budget_kbps` from the host config, default and maximum 300 kbps
  (37 500 bytes per second). Capacity: 16 KiB.
- Counted bytes are wire estimates: frame or datagram bytes plus 40 bytes of
  IP/TCP header per TCP frame, or 28 bytes of IP/UDP header per datagram.
- Outgoing TCP frames wait for tokens (back-pressure on the sender).
- Outgoing UDP audio packets are dropped when no tokens are available.
- Incoming bytes are charged when received (the bucket may go into debt), so
  heavy PSP-to-host traffic slows host-to-PSP traffic and the total stays
  under the cap.

## Versioning

The protocol version is the `version` byte of discovery and handshake messages.
A peer with an unsupported version is rejected during the handshake (status 2).
New message types can be added within version 1 because unknown types are
ignored; changing an existing payload requires version 2.
