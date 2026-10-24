# Third-party components

## LAME (host, MP3 encoder)

- License: GNU Library General Public License 2.0 or later (LGPL-2.0-or-later). Full text:
  <https://www.gnu.org/licenses/old-licenses/lgpl-2.0.txt>; the LAME source and its `COPYING` file ship inside the
  `mp3lame-sys` crate (version pinned in `Cargo.lock`).
- Use: `host/src/audio/encode_mp3.rs` encodes the PC-speaker stream through `mp3lame-encoder` / `mp3lame-sys`.
  `mp3lame-sys` compiles LAME from source and links it statically into `companion-host.exe`.
- LAME is copyright its authors (<https://lame.sourceforge.io/>). It is used unmodified.

### Relinking with a modified LAME

The LGPL lets you replace LAME in `companion-host.exe` with your own version. The complete host source is in this
repository under the MIT license, so:

1. Clone this repository at the release tag.
2. Point Cargo at your LAME: `cargo vendor` (or a `[patch.crates-io]` entry in `Cargo.toml`) with your modified
   `mp3lame-sys`, or edit the vendored LAME sources inside it.
3. `cargo build --release -p companion-host`. The resulting `target/release/companion-host.exe` uses your LAME.

## GNU Unifont subset (PSP font)

`psp/font_vi.c` is generated from `psp/assets/font-vi.hex`, a GNU Unifont subset under the SIL Open Font License 1.1
(`psp/assets/OFL-1.1.txt`).
