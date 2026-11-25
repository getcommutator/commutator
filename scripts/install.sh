#!/bin/sh
# Install Commutator's engine on a Linux server and add it to your account:
#
#   curl -fsSL <server>/install.sh | sh -s -- --code <pair code>
#
# The server serves this file with its own address filled in (SERVER below);
# the pair code comes from Add a server in the desktop app (or any signed-in
# device's POST /devices/pair). Downloads the latest release's `commutator-<version>-linux-<arch>`
# build (no desktop window) to ~/.local/bin, signs in with the code, and runs
# it as a systemd user service that survives logout.
set -eu

SERVER="${COMMUTATOR_SERVER_URL:-__SERVER__}"
REPO="${COMMUTATOR_RELEASES_REPO:-getcommutator/commutator}"
BIN_DIR="${COMMUTATOR_BIN_DIR:-$HOME/.local/bin}"
CODE=""

while [ $# -gt 0 ]; do
  case "$1" in
    --code) CODE="${2:-}"; shift 2 ;;
    --code=*) CODE="${1#--code=}"; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "this installs the Linux engine; on a Mac, install the app"
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) die "no build for $(uname -m)" ;;
esac
command -v curl >/dev/null || die "curl is needed"
command -v systemctl >/dev/null || die "systemd is needed to run the engine as a service"

say "Finding the latest release of $REPO…"
URL=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep -o "\"browser_download_url\": *\"[^\"]*-linux-$ARCH\.tar\.gz\"" \
  | sed 's/.*"\(https[^"]*\)"$/\1/' | head -n 1)
[ -n "$URL" ] || die "the latest release has no Linux $ARCH build"

say "Downloading ${URL##*/}…"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "$URL" -o "$TMP/commutator.tar.gz"
tar -xzf "$TMP/commutator.tar.gz" -C "$TMP"
mkdir -p "$BIN_DIR"
# Stop a running service first: its binary is about to be replaced.
systemctl --user stop commutator.service 2>/dev/null || true
install -m 755 "$TMP/commutator" "$BIN_DIR/commutator"
"$BIN_DIR/commutator" --version

if [ -n "$CODE" ]; then
  say "Signing in to $SERVER…"
  "$BIN_DIR/commutator" login "$SERVER" --code "$CODE"
else
  echo "No --code: sign in later with \`commutator login $SERVER\`."
fi

say "Starting the engine…"
"$BIN_DIR/commutator" daemon install
# A user service stops at logout without lingering; usually allowed for yourself.
loginctl enable-linger "$(id -un)" 2>/dev/null \
  || echo "note: \`loginctl enable-linger $(id -un)\` failed; run it with sudo so the engine outlives your SSH session"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: add $BIN_DIR to PATH to run \`commutator\` yourself" ;;
esac
say "Done: this server shows up in your devices as \"$(hostname)\"."
echo "Agents (Claude Code, Codex, …) run here too: install and sign them in on this server."
