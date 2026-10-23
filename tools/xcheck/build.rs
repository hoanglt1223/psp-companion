fn main() {
    for f in ["crypto.c", "crypto.h", "protocol.c", "protocol.h"] {
        println!("cargo:rerun-if-changed=../../psp/{f}");
    }
    cc::Build::new()
        .files(["../../psp/crypto.c", "../../psp/protocol.c"])
        .include("../../psp")
        .warnings_into_errors(true)
        .compile("pspc_portable");
}
