#!/usr/bin/env bash
# Install circulartrackpad: build the binary, install it to /usr/local/bin,
# and set up a udev rule granting the active-seat user access to the
# trackpad and /dev/uinput via POSIX ACLs (systemd uaccess).
set -euo pipefail

BIN_NAME="circulartrackpad"
BIN_DEST="/usr/local/bin/${BIN_NAME}"
RULE_PATH="/etc/udev/rules.d/70-circulartrackpad.rules"

# Synaptics TM3562-003 HID IDs (from /proc/bus/input/devices)
VENDOR_ID="06cb"
PRODUCT_ID="cdea"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

echo "==> Building release binary"
cargo build --release

echo "==> Installing ${BIN_DEST} (sudo)"
sudo install -m 0755 "target/release/${BIN_NAME}" "${BIN_DEST}"

echo "==> Writing udev rule to ${RULE_PATH} (sudo)"
sudo tee "${RULE_PATH}" > /dev/null <<EOF
# circulartrackpad: grant the active local-seat user access to the
# Panasonic Let's Note circular trackpad and /dev/uinput via ACLs.
# Installed by install.sh.

KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"
SUBSYSTEM=="input", ATTRS{id/vendor}=="${VENDOR_ID}", ATTRS{id/product}=="${PRODUCT_ID}", TAG+="uaccess"
EOF

echo "==> Reloading udev rules"
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input --subsystem-match=misc

echo
echo "Done. You may need to log out and back in once for the ACLs to apply."
echo "Then run: ${BIN_NAME} --help"
