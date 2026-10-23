//! `pspc-fontgen --subset <unifont.hex>` writes `psp/assets/font-vi.hex` (the glyphs the PSP needs).
//! `pspc-fontgen` regenerates `psp/font_vi.c` from that subset.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pspc_fontgen::{generate_c, parse_hex, subset};

fn psp_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../psp")
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = psp_dir();
    let subset_path = dir.join("assets/font-vi.hex");
    match args.as_slice() {
        [flag, full] if flag == "--subset" => {
            let text = std::fs::read_to_string(full).map_err(|e| format!("cannot read {full}: {e}"))?;
            std::fs::write(&subset_path, subset(&parse_hex(&text)?)).map_err(|e| e.to_string())?;
        }
        [] => {}
        _ => return Err("usage: pspc-fontgen [--subset <unifont.hex>]".into()),
    }
    let text =
        std::fs::read_to_string(&subset_path).map_err(|e| format!("cannot read {}: {e}", subset_path.display()))?;
    std::fs::write(dir.join("font_vi.c"), generate_c(&parse_hex(&text)?)).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
