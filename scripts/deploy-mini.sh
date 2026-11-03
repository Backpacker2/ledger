#!/usr/bin/env bash
# Puts the mobile version of Ledger on a machine at home (a Mac mini, or a Linux box) and keeps it running there.
#
#   scripts/deploy-mini.sh <ssh-host>        build, copy, (re)start the service, put it on the tailnet
#   scripts/deploy-mini.sh --package         only build the bundle into dist/ (LEDGER_OS and LEDGER_ARCH pick which)
#
# <ssh-host> is anything `ssh` accepts: user@address, the machine's Tailscale name, or a Host from ~/.ssh/config.
# The script asks the machine what it is (macOS or Linux, Intel or ARM) and builds for that. The machine needs
# nothing installed but Tailscale (and SSH). Linux builds go through Docker (scripts/build-linux-server.sh).
#
# Settings, as environment variables when running this script:
#   LEDGER_SSH_KEY         the key to log in with (default: ssh's own choice)
#   LEDGER_REMOTE_DIR      where it lives there (default: ~/ledger-server)
#   LEDGER_REMOTE_VAULT    the vault there (default: ~/ledger-vault; created empty, fill it with Settings → Sync)
#   LEDGER_PORT            the local port it listens on (default: 8787)
#   LEDGER_ALLOWED_LOGINS  only these Tailscale logins get in (comma-separated; default: everyone on your tailnet)
#   LEDGER_SKIP_SERVE=1    don't run `tailscale serve` (when you set it up yourself)
#   LEDGER_OS, LEDGER_ARCH for --package: macos|linux and x86_64|aarch64 (default: macos, x86_64)
#
# It keeps running with a LaunchAgent on macOS (~/Library/LaunchAgents/nl.backpacker.ledger-server.plist; turn on
# automatic login so it comes back after a restart), or a systemd user service on Linux
# (~/.config/systemd/user/ledger-server.service, with lingering so it runs without anyone logged in).

set -euo pipefail

PORT="${LEDGER_PORT:-8787}"
REMOTE_DIR="${LEDGER_REMOTE_DIR:-~/ledger-server}"
REMOTE_VAULT="${LEDGER_REMOTE_VAULT:-~/ledger-vault}"
ALLOWED="${LEDGER_ALLOWED_LOGINS:-}"
LABEL="nl.backpacker.ledger-server"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33m(!) %s\033[0m\n' "$*"; }
die() { printf 'deploy-mini: %s\n' "$*" >&2; exit 1; }
usage() { sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

SSH_OPTS=(-o ConnectTimeout=10)
[[ -n "${LEDGER_SSH_KEY:-}" ]] && SSH_OPTS+=(-i "$LEDGER_SSH_KEY" -o IdentitiesOnly=yes)
remote() { ssh "${SSH_OPTS[@]}" "$HOST" "$@"; }

case "${1:-}" in
	-h | --help) usage 0 ;;
	--package) HOST="" ;;
	"") usage 1 ;;
	-*) die "unknown option $1" ;;
	*) HOST="$1" ;;
esac

# ---- what to build for ---------------------------------------------------------------------------------

if [[ -n "$HOST" ]]; then
	say "Asking $HOST what it is"
	read -r KERNEL MACHINE < <(remote 'uname -sm') || die "can't reach $HOST over ssh"
	case "$KERNEL" in Darwin) OS=macos ;; Linux) OS=linux ;; *) die "$HOST runs $KERNEL; only macOS and Linux are supported" ;; esac
	case "$MACHINE" in x86_64 | amd64) ARCH=x86_64 ;; arm64 | aarch64) ARCH=aarch64 ;; *) die "$HOST is $MACHINE; only x86_64 and ARM are supported" ;; esac
	echo "    $OS, $ARCH"
else
	OS="${LEDGER_OS:-macos}"
	ARCH="${LEDGER_ARCH:-x86_64}"
fi
BUNDLE="$ROOT/dist/ledger-server-$OS-$ARCH"

# ---- build ---------------------------------------------------------------------------------------------

say "Building the screens"
(cd "$ROOT" && pnpm run build >/dev/null)

say "Building ledger-server for $OS $ARCH"
if [[ "$OS" == macos ]]; then
	TARGET="$ARCH-apple-darwin"
	rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
	cargo build --release --locked --no-default-features --features server --bin ledger-server --target "$TARGET" \
		--manifest-path "$ROOT/src-tauri/Cargo.toml"
	BINARY="$ROOT/src-tauri/target/$TARGET/release/ledger-server"
else
	BINARY=$("$ROOT/scripts/build-linux-server.sh" "$ARCH" | tail -1)
fi

say "Packaging $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE"
cp "$BINARY" "$BUNDLE/"
cp -R "$ROOT/build" "$BUNDLE/web"

# Started by the service: reads the settings next to it, then becomes the server.
cat >"$BUNDLE/run.sh" <<'EOF'
#!/bin/sh
# Started by launchd or systemd (see scripts/deploy-mini.sh). Settings come from ledger-server.env next to this file.
cd "$(dirname "$0")" || exit 1
set -a
[ -f ./ledger-server.env ] && . ./ledger-server.env
set +a
export LEDGER_STATIC="${LEDGER_STATIC:-$PWD/web}"
exec ./ledger-server
EOF
chmod +x "$BUNDLE/run.sh" "$BUNDLE/ledger-server"

if [[ -z "$HOST" ]]; then
	say "Done: $BUNDLE"
	exit 0
fi

# ---- install there -------------------------------------------------------------------------------------

say "Copying to $HOST:$REMOTE_DIR"
remote "mkdir -p $REMOTE_DIR $REMOTE_VAULT"
# The settings file there is kept: it is written below only the first time.
rsync -az --delete --exclude ledger-server.env --exclude logs -e "ssh ${SSH_OPTS[*]}" "$BUNDLE/" "$HOST:$REMOTE_DIR/"

say "Writing the settings"
remote "bash -s" <<EOF
set -euo pipefail
dir=\$(cd $REMOTE_DIR && pwd)
vault=\$(cd $REMOTE_VAULT && pwd)
mkdir -p "\$dir/logs"
if [[ ! -f "\$dir/ledger-server.env" ]]; then
	cat >"\$dir/ledger-server.env" <<ENV
# Settings for ledger-server (see README → The mobile server). Restart it after a change (see that section).
LEDGER_VAULT=\$vault
LEDGER_CONFIG_DIR=\$dir/config
LEDGER_SERVER_ADDR=127.0.0.1:$PORT
LEDGER_ALLOWED_LOGINS=$ALLOWED
ENV
fi
EOF

if [[ "$OS" == macos ]]; then
	say "Installing the LaunchAgent"
	remote "bash -s" <<EOF
set -euo pipefail
dir=\$(cd $REMOTE_DIR && pwd)
plist="\$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "\$HOME/Library/LaunchAgents"
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
else
	say "Installing the systemd user service"
	remote "bash -s" <<EOF
set -euo pipefail
dir=\$(cd $REMOTE_DIR && pwd)
unit_dir="\$HOME/.config/systemd/user"
mkdir -p "\$unit_dir"
cat >"\$unit_dir/ledger-server.service" <<UNIT
[Unit]
Description=Ledger mobile server (ledger-server)
After=network-online.target

[Service]
ExecStart=\$dir/run.sh
WorkingDirectory=\$dir
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
UNIT
systemctl --user daemon-reload
systemctl --user enable ledger-server.service >/dev/null 2>&1
systemctl --user restart ledger-server.service
# Lingering keeps user services running without anyone logged in, and starts them at boot.
if [[ "\$(loginctl show-user "\$USER" -p Linger --value 2>/dev/null)" != yes ]]; then
	loginctl enable-linger "\$USER" 2>/dev/null || echo "LINGER_NEEDED"
fi
EOF
fi

say "Waiting for it to answer"
ok=""
for _ in $(seq 1 20); do
	if remote "curl -fsS http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1; then ok=1; break; fi
	sleep 1
done
[[ -n "$ok" ]] || die "it didn't start; on $HOST see $REMOTE_DIR/logs/ (macOS) or 'journalctl --user -u ledger-server' (Linux)"
remote "loginctl show-user \"\$USER\" -p Linger --value 2>/dev/null | grep -qx yes" 2>/dev/null || [[ "$OS" == macos ]] ||
	warn "It runs, but stops when you log out and won't start at boot. Once, on $HOST: sudo loginctl enable-linger \$USER"

if [[ "${LEDGER_SKIP_SERVE:-}" != "1" ]]; then
	say "Putting it on the tailnet"
	if ! remote "bash -s" <<EOF; then
set -euo pipefail
ts=\$(command -v tailscale || echo /Applications/Tailscale.app/Contents/MacOS/Tailscale)
[[ -x "\$ts" ]] || { echo "Tailscale isn't installed on this machine"; exit 1; }
"\$ts" serve --bg $PORT >/dev/null
"\$ts" serve status
EOF
		warn "tailscale serve didn't work. On $HOST, once: sudo tailscale set --operator=\$USER (Linux), then run this again;"
		warn "or there: tailscale serve --bg $PORT. The server runs locally on 127.0.0.1:$PORT meanwhile."
		exit 0
	fi
fi

say "Done. Open the https://….ts.net address above on your phone, then Share → Add to Home Screen."
