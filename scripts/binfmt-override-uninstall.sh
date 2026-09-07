#!/usr/bin/env bash
# Reverts scripts/binfmt-override-install.sh: removes the /etc/binfmt.d
# override (unmasking the vendor file underneath it again), restarts
# systemd-binfmt.service, then verifies the live registration actually
# matches what was backed up before the override went in. The backup is
# only for that verification — if it matches, it's deleted; if it doesn't
# (e.g. a package update changed the vendor file while the override was
# active), it's left in place and shown to you rather than guessed at.
set -euo pipefail

BACKUP_DIR="$HOME/.config/iprolaunch/binfmt-override"
STATE_FILE="$BACKUP_DIR/state"
ORIGINAL_FILE="$BACKUP_DIR/original.conf"

if [[ ! -f "$STATE_FILE" ]]; then
    echo "Nothing to revert — no $STATE_FILE (never installed, or already reverted)."
    exit 0
fi

# shellcheck source=/dev/null
source "$STATE_FILE" # provides: basename=, entry=, vendor_file=

OVERRIDE_FILE="/etc/binfmt.d/$basename"

if [[ ! -e "$OVERRIDE_FILE" ]]; then
    echo "note: $OVERRIDE_FILE is already gone — removing stale backup state."
    rm -f "$STATE_FILE" "$ORIGINAL_FILE"
    exit 0
fi

echo "==> Removing $OVERRIDE_FILE (sudo)..."
sudo rm -f "$OVERRIDE_FILE"

echo "==> Restarting systemd-binfmt.service (sudo)..."
sudo systemctl restart systemd-binfmt.service

echo "==> Verifying against the pre-override backup..."
ORIGINAL_INTERPRETER="$(awk -F: -v e=":${entry}:" 'index($0, e) == 1 {print $7}' "$ORIGINAL_FILE" 2>/dev/null || true)"
LIVE_INTERPRETER="$(awk '/^interpreter /{print $2}' "/proc/sys/fs/binfmt_misc/${entry}" 2>/dev/null || true)"

if [[ -n "$LIVE_INTERPRETER" && "$LIVE_INTERPRETER" == "$ORIGINAL_INTERPRETER" ]]; then
    echo "OK: /proc/sys/fs/binfmt_misc/${entry} matches the pre-override original ($LIVE_INTERPRETER)."
    echo "==> Removing the backup."
    rm -f "$STATE_FILE" "$ORIGINAL_FILE"
else
    echo "WARNING: post-revert interpreter doesn't match the backed-up original:" >&2
    echo "           live:     ${LIVE_INTERPRETER:-<none>}" >&2
    echo "           original: ${ORIGINAL_INTERPRETER:-<none>}" >&2
    echo "         Something else (e.g. a package update to $vendor_file) may have changed" >&2
    echo "         in between. Backup kept at: $ORIGINAL_FILE — compare by hand before deleting." >&2
    exit 1
fi
