#!/usr/bin/env sh
# Builds out/packages/companion-host-<version>-linux-x86_64.tar.gz: a headless host (no GTK needed), the systemd user
# unit, install.sh, the docs and the license notices.
# Usage: host/packaging/linux/package.sh [version]   (default: the crate version)
set -eu

root="$(cd "$(dirname "$0")/../../.." && pwd)"
here="$root/host/packaging/linux"
version="${1:-v$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/host/Cargo.toml" | head -n 1)}"
arch="$(uname -m)"
name="companion-host-$version-linux-$arch"
stage="$root/out/packages/$name"

cargo build --manifest-path "$root/Cargo.toml" --release --locked -p companion-host --no-default-features

rm -rf "$stage"
mkdir -p "$stage/docs"
install -m 0755 "$root/target/release/companion-host" "$stage/companion-host"
install -m 0755 "$here/install.sh" "$stage/install.sh"
install -m 0644 "$here/companion-host.service" "$stage/companion-host.service"
install -m 0644 "$root/LICENSE" "$root/THIRD_PARTY.md" "$stage/"
install -m 0644 "$root"/docs/user/*.md "$stage/docs/"

tar -C "$root/out/packages" -czf "$root/out/packages/$name.tar.gz" "$name"
rm -rf "$stage"
echo "$root/out/packages/$name.tar.gz"
