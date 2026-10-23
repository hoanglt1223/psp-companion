const PSP_SOURCES: &[&str] = &["crypto.c", "protocol.c", "font.c", "font_vi.c", "image.c", "jitter.c"];
const PSP_HEADERS: &[&str] = &["crypto.h", "protocol.h", "font.h", "image.h", "jitter.h"];

fn main() {
    for f in PSP_SOURCES.iter().chain(PSP_HEADERS) {
        println!("cargo:rerun-if-changed=../../psp/{f}");
    }
    println!("cargo:rerun-if-changed=c/shim.c");
    cc::Build::new()
        .files(PSP_SOURCES.iter().map(|f| format!("../../psp/{f}")))
        .file("c/shim.c")
        .include("../../psp")
        .warnings_into_errors(true)
        .compile("pspc_portable");
}
