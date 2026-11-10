#!/bin/sh
# Installs the latest aweave release for this machine.
#   curl -fsSL https://aweave.pages.dev/install.sh | sh
# Installs to $AWEAVE_INSTALL_DIR (default: ~/.local/bin). Never uses sudo. Verifies the SHA-256 that the release
# publishes next to every archive. Windows: download the .zip from the releases page instead.
set -eu

REPO="DanielHromadka/aweave"
DIR="${AWEAVE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'aweave-install: %s\n' "$*" >&2; }
fail() { say "$*"; exit 1; }

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) fail "unsupported system $(uname -s); see https://github.com/$REPO/releases/latest" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) fail "unsupported architecture $(uname -m)" ;;
esac
# An x64 shell under Rosetta on an Apple silicon Mac still gets the native build.
if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch=aarch64
fi

asset="aweave-$arch-$os.tar.gz"
url="https://github.com/$REPO/releases/latest/download/$asset"
command -v curl > /dev/null || fail "curl is required"
command -v tar > /dev/null || fail "tar is required"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $asset"
curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset" "$url" || fail "download failed: $url"
curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset.sha256" "$url.sha256" || fail "checksum download failed"

expected=$(awk '{print $1}' "$tmp/$asset.sha256")
if command -v sha256sum > /dev/null; then
  actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
elif command -v shasum > /dev/null; then
  actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
else
  fail "need sha256sum or shasum to verify the download"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset (expected $expected, got $actual)"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$DIR"
install -m 0755 "$tmp/aweave" "$DIR/aweave" 2> /dev/null || { cp "$tmp/aweave" "$DIR/aweave" && chmod 0755 "$DIR/aweave"; }

say "installed $("$DIR/aweave" --version) to $DIR/aweave"
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) say "add $DIR to your PATH, e.g.: echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.profile" ;;
esac
