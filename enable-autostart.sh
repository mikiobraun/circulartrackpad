#!/usr/bin/env bash
# Install and enable a systemd user service that starts circulartrackpad
# at login. Any arguments passed to this script are forwarded to the
# daemon, e.g.:
#
#     ./enable-autostart.sh -p 2.0 -s 6 -r 0.6
#
# To change options later, edit ~/.config/systemd/user/circulartrackpad.service
# and run: systemctl --user daemon-reload && systemctl --user restart circulartrackpad
set -euo pipefail

BIN="/usr/local/bin/circulartrackpad"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT_PATH="${UNIT_DIR}/circulartrackpad.service"

if [[ ! -x "${BIN}" ]]; then
    echo "error: ${BIN} not found. Run ./install.sh first." >&2
    exit 1
fi

DAEMON_ARGS="$*"
EXEC_START="${BIN}${DAEMON_ARGS:+ ${DAEMON_ARGS}}"

echo "==> Writing ${UNIT_PATH}"
mkdir -p "${UNIT_DIR}"
cat > "${UNIT_PATH}" <<EOF
[Unit]
Description=Circular trackpad daemon

[Service]
ExecStart=${EXEC_START}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

echo "==> Reloading systemd user daemon"
systemctl --user daemon-reload

echo "==> Enabling and starting circulartrackpad.service"
systemctl --user enable --now circulartrackpad.service

echo
echo "Status:"
systemctl --user --no-pager status circulartrackpad.service || true
