#!/bin/sh
# Build and install agentd for the current user, from this checkout.
set -eu

cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found; install Rust from https://rustup.rs" >&2
    exit 1
fi

cargo install --path . --locked --force

# The dashboard is part of the install: a user service, enabled at boot.
if [ -z "${HOME_REPO_PATH:-}" ]; then
    echo "warning: HOME_REPO_PATH unset; skipping the dashboard service" >&2
    echo "         rerun with HOME_REPO_PATH set to install and start it" >&2
elif [ ! -d "$HOME_REPO_PATH" ]; then
    echo "error: HOME_REPO_PATH is not a directory: $HOME_REPO_PATH" >&2
    exit 1
elif ! systemctl --user show-environment >/dev/null 2>&1; then
    echo "warning: no systemd user session; skipping the dashboard service" >&2
else
    home=$(cd "$HOME_REPO_PATH" && pwd -P)
    unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$unit_dir"
    cat > "$unit_dir/agentd-dashboard.service" <<EOF
[Unit]
Description=agentd dashboard

[Service]
Environment="HOME_REPO_PATH=$home"
ExecStart="$(command -v agentd)" dashboard
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable agentd-dashboard.service
    systemctl --user restart agentd-dashboard.service
    # Start at boot, not only at login.
    loginctl enable-linger "$USER" 2>/dev/null \
        || echo "warning: could not enable linger; the dashboard starts at login only" >&2
    sleep 1
    if systemctl --user is-active --quiet agentd-dashboard.service; then
        echo "dashboard: http://127.0.0.1:7878"
    else
        echo "warning: dashboard did not start; see: journalctl --user -u agentd-dashboard" >&2
    fi
fi

echo "installed; check with: agentd help"
