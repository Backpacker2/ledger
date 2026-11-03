#!/usr/bin/env bash
# Puts the mobile version of Ledger on a Mac at home (an Intel Mac mini, say) and keeps it running there.
#
#   scripts/deploy-mini.sh <ssh-host>        build, copy, (re)start the service, put it on the tailnet
#   scripts/deploy-mini.sh --package         only build the bundle into dist/ledger-server-macos-<arch>/
#
# <ssh-host> is anything `ssh` accepts: the Mac's Tailscale name (with Tailscale SSH or Remote Login on), or a
# Host from ~/.ssh/config. The other Mac needs nothing installed but Tailscale.
#
# Settings, as environment variables when running this script:
#   LEDGER_ARCH            x86_64 (default: an Intel Mac) or aarch64 (Apple silicon)
#   LEDGER_REMOTE_DIR      where it lives on the other Mac (default: ~/ledger-server)
#   LEDGER_REMOTE_VAULT    the vault there (default: ~/ledger-vault; created empty, fill it with Settings → Sync)
#   LEDGER_PORT            the local port it listens on (default: 8787)
#   LEDGER_ALLOWED_LOGINS  only these Tailscale logins get in (comma-separated; default: everyone on your tailnet)
#   LEDGER_SKIP_SERVE=1    don't run `tailscale serve` (when you set it up yourself)
#
# On the other Mac it installs a LaunchAgent (~/Library/LaunchAgents/nl.backpacker.ledger-server.plist), which
# starts the server at login and restarts it when it stops. For it to run after a restart without anyone at the
# keyboard, turn on automatic login for that user (System Settings → Users & Groups).

set -euo pipefail

ARCH="${LEDGER_ARCH:-x86_64}"
PORT="${LEDGER_PORT:-8787}"
REMOTE_DIR="${LEDGER_REMOTE_DIR:-~/ledger-server}"
REMOTE_VAULT="${LEDGER_REMOTE_VAULT:-~/ledger-vault}"
ALLOWED="${LEDGER_ALLOWED_LOGINS:-}"
LABEL="nl.backpacker.ledger-server"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${ARCH}-apple-darwin"
BUNDLE="$ROOT/dist/ledger-server-macos-$ARCH"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'deploy-mini: %s\n' "$*" >&2; exit 1; }

usage() { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

case "${1:-}" in
	-h | --help) usage 0 ;;
	--package) HOST="" ;;
	"") usage 1 ;;
	-*) die "unknown option $1" ;;
	*) HOST="$1" ;;
esac

# ---- build ---------------------------------------------------------------------------------------------

say "Building the screens"
(cd "$ROOT" && pnpm run build >/dev/null)

say "Building ledger-server for $TARGET"
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
cargo build --release --locked --features server --bin ledger-server --target "$TARGET" \
	--manifest-path "$ROOT/src-tauri/Cargo.toml"

say "Packaging $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE"
cp "$ROOT/src-tauri/target/$TARGET/release/ledger-server" "$BUNDLE/"
cp -R "$ROOT/build" "$BUNDLE/web"

# Started by launchd: reads the settings next to it, then becomes the server. Paths are relative to itself.
cat >"$BUNDLE/run.sh" <<'EOF'
#!/bin/zsh
# Started by launchd (see deploy-mini.sh). Settings come from ledger-server.env next to this file.
cd "${0:A:h}"
set -a
[[ -f ./ledger-server.env ]] && source ./ledger-server.env
set +a
export LEDGER_STATIC="${LEDGER_STATIC:-$PWD/web}"
exec ./ledger-server
EOF
chmod +x "$BUNDLE/run.sh" "$BUNDLE/ledger-server"

if [[ -z "$HOST" ]]; then
	say "Done: $BUNDLE"
	exit 0
fi

# ---- install on the other Mac ---------------------------------------------------------------------------

say "Checking $HOST"
ssh -o ConnectTimeout=10 "$HOST" 'uname -sm' | grep -q "Darwin" || die "$HOST isn't a Mac (or can't be reached over ssh)"
REMOTE_ARCH=$(ssh "$HOST" 'uname -m')
[[ "$REMOTE_ARCH" == "$ARCH" || ("$REMOTE_ARCH" == "arm64" && "$ARCH" == "aarch64") ]] ||
	die "$HOST is $REMOTE_ARCH; run again with LEDGER_ARCH=$([[ $REMOTE_ARCH == arm64 ]] && echo aarch64 || echo x86_64)"

say "Copying to $HOST:$REMOTE_DIR"
ssh "$HOST" "mkdir -p $REMOTE_DIR $REMOTE_VAULT"
# The settings file on the other Mac is kept: it is written below only the first time.
rsync -az --delete --exclude ledger-server.env --exclude logs "$BUNDLE/" "$HOST:$REMOTE_DIR/"

say "Writing the settings and the LaunchAgent"
ssh "$HOST" "bash -s" <<EOF
set -euo pipefail
dir=\$(cd $REMOTE_DIR && pwd)
vault=\$(cd $REMOTE_VAULT && pwd)
env_file="\$dir/ledger-server.env"
if [[ ! -f "\$env_file" ]]; then
	cat >"\$env_file" <<ENV
# Settings for ledger-server (see README → The mobile server). Restart it after a change:
#   launchctl kickstart -k gui/\$(id -u)/$LABEL
LEDGER_VAULT=\$vault
LEDGER_SERVER_ADDR=127.0.0.1:$PORT
LEDGER_ALLOWED_LOGINS=$ALLOWED
ENV
fi
plist="\$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "\$HOME/Library/LaunchAgents" "\$dir/logs"
cat >"\$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$LABEL</string>
	<key>ProgramArguments</key><array><string>\$dir/run.sh</string></array>
	<key>WorkingDirectory</key><string>\$dir</string>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><true/>
	<key>ThrottleInterval</key><integer>10</integer>
	<key>ProcessType</key><string>Background</string>
	<key>StandardOutPath</key><string>\$dir/logs/stdout.log</string>
	<key>StandardErrorPath</key><string>\$dir/logs/stderr.log</string>
</dict>
</plist>
PLIST
domain="gui/\$(id -u)"
launchctl bootout "\$domain/$LABEL" 2>/dev/null || true
launchctl bootstrap "\$domain" "\$plist"
launchctl kickstart -k "\$domain/$LABEL"
EOF

say "Waiting for it to answer"
for _ in $(seq 1 20); do
	if ssh "$HOST" "curl -fsS http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1; then
		break
	fi
	sleep 1
done
ssh "$HOST" "curl -fsS http://127.0.0.1:$PORT/api/health" >/dev/null ||
	die "it didn't start; see $REMOTE_DIR/logs/stderr.log on $HOST"

if [[ "${LEDGER_SKIP_SERVE:-}" != "1" ]]; then
	say "Putting it on the tailnet"
	# The Mac App Store / standalone Tailscale app keeps its command line inside the app.
	ssh "$HOST" "bash -s" <<EOF
set -euo pipefail
ts=\$(command -v tailscale || echo /Applications/Tailscale.app/Contents/MacOS/Tailscale)
"\$ts" serve --bg $PORT >/dev/null
"\$ts" serve status
EOF
fi

say "Done. Open the https://….ts.net address above on your phone, then Share → Add to Home Screen."
