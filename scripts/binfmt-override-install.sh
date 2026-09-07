#!/usr/bin/env bash
# Routes Windows .exe execution (the kernel's binfmt_misc, normally MZ-header
# -> wine directly) through iprolaunch instead, so a `+x` exe run directly
# (e.g. `./game.exe`, or a file manager's "execute" behavior) launches
# through iprolaunch's own profile/config handling rather than bare wine.
#
# How: writes /etc/binfmt.d/<same filename as the vendor's *.conf> — same
# filename in a higher-precedence directory fully masks the vendor file
# (binfmt.d(5), "Files in /etc/ override files with the same name..."),
# rather than trying to register a second competing entry. Only the
# interpreter changes; the vendor's own type/offset/magic/mask/flags are
# reused as-is. See README.md's FAQ for the manual equivalent and caveats
# (this only lasts until the /etc file is removed; it survives reboots,
# unlike toggling /proc/sys/fs/binfmt_misc/<name> by hand).
#
# Safe to re-run: if already installed, this updates the interpreter path in
# place (no need to uninstall first) — useful after moving/reinstalling the
# iprolaunch binary. The one-time backup of the vendor file is never
# overwritten by an update, only by uninstall.sh once it's confirmed restored.
#
# Usage: binfmt-override-install.sh [--bin PATH] [--entry NAME]
#   --bin PATH    absolute (or relative) path to the iprolaunch binary to
#                 register. Default: whichever exists first of —
#                   1) ~/.config/iprolaunch/bin-path, which iprolaunch
#                      writes on every run with its own resolved path
#                      (record_bin_path() in src/main.rs) — the real fix
#                      for "the binary can be anywhere," just run any
#                      `iprolaunch ...` command once first if it's missing;
#                   2) `command -v iprolaunch` ($PATH lookup).
#   --entry NAME  binfmt_misc entry name to target. Default: DOSWin.
set -euo pipefail

usage() {
    awk '/^#!/{next} /^#$/{print ""; next} /^# /{sub(/^# ?/,""); print; next} {exit}' "$0"
}

ENTRY_NAME="DOSWin"
BIN_OVERRIDE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
    --bin)
        BIN_OVERRIDE="${2:?--bin needs a path}"
        shift 2
        ;;
    --entry)
        ENTRY_NAME="${2:?--entry needs a name}"
        shift 2
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "unknown argument: $1 (see --help)" >&2
        exit 1
        ;;
    esac
done

BACKUP_DIR="$HOME/.config/iprolaunch/binfmt-override"
STATE_FILE="$BACKUP_DIR/state"
ORIGINAL_FILE="$BACKUP_DIR/original.conf"
BIN_PATH_FILE="$HOME/.config/iprolaunch/bin-path"
OVERRIDE_DIR="/etc/binfmt.d"
VENDOR_DIRS=(/run/binfmt.d /usr/local/lib/binfmt.d /usr/lib/binfmt.d /lib/binfmt.d)

# --- Step 1: are we updating an existing install, or doing a fresh one? ---
# Only computes paths / reads state here — no writes yet, so a later failure
# (e.g. an invalid --bin) leaves nothing behind to clean up.
ALREADY_INSTALLED=false
basename="" entry="" vendor_file="" # populated by sourcing $STATE_FILE below
if [[ -f "$STATE_FILE" ]]; then
    # shellcheck source=/dev/null
    source "$STATE_FILE"
    if [[ "$entry" != "$ENTRY_NAME" ]]; then
        echo "error: already installed for entry '$entry', but --entry $ENTRY_NAME was given." >&2
        echo "       omit --entry to update the existing '$entry' override, or uninstall first." >&2
        exit 1
    fi
    ALREADY_INSTALLED=true
    OVERRIDE_FILE="$OVERRIDE_DIR/$basename"
    echo "==> Already installed, overriding $vendor_file — will update in place."
fi

# --- Step 2: resolve the iprolaunch binary to register. It can live
# anywhere, so this never guesses beyond $PATH and (on an update) whatever's
# already configured — an explicit --bin always wins. ---
echo "==> Resolving the iprolaunch binary to register..."
if [[ -n "$BIN_OVERRIDE" ]]; then
    IPROLAUNCH_BIN="$(readlink -f "$BIN_OVERRIDE" 2>/dev/null || true)"
    if [[ -z "$IPROLAUNCH_BIN" || ! -x "$IPROLAUNCH_BIN" ]]; then
        echo "error: --bin $BIN_OVERRIDE isn't an existing, executable file." >&2
        exit 1
    fi
elif [[ -r "$BIN_PATH_FILE" ]] && reported="$(<"$BIN_PATH_FILE")" && [[ -x "$reported" ]]; then
    # iprolaunch writes its own resolved path here on every run (see
    # record_bin_path() in src/main.rs) — ground truth for "whichever copy
    # you actually run," regardless of $PATH. Just run `iprolaunch` once
    # (any command) if this doesn't exist yet or looks stale.
    IPROLAUNCH_BIN="$reported"
    echo "    (found via $BIN_PATH_FILE, self-reported by iprolaunch's own last run)"
elif found="$(command -v iprolaunch || true)" && [[ -n "$found" ]]; then
    IPROLAUNCH_BIN="$found"
elif $ALREADY_INSTALLED && [[ -e "$OVERRIDE_FILE" ]]; then
    IPROLAUNCH_BIN="$(awk -F: '{print $7}' "$OVERRIDE_FILE" | grep -v '^$' | head -n1)"
    echo "    'iprolaunch' isn't on \$PATH and no --bin was given — reusing the currently"
    echo "    configured path: $IPROLAUNCH_BIN"
else
    echo "error: couldn't find iprolaunch: no --bin given, $BIN_PATH_FILE is missing or" >&2
    echo "       stale (run any 'iprolaunch ...' command once to refresh it), and it's" >&2
    echo "       not on \$PATH. Point at it explicitly instead: $0 --bin /path/to/iprolaunch" >&2
    exit 1
fi
echo "    -> $IPROLAUNCH_BIN"

if $ALREADY_INSTALLED && [[ -e "$OVERRIDE_FILE" ]]; then
    CURRENT="$(awk -F: '{print $7}' "$OVERRIDE_FILE" | grep -v '^$' | head -n1 || true)"
    if [[ "$CURRENT" == "$IPROLAUNCH_BIN" ]]; then
        echo "==> Already up to date ($IPROLAUNCH_BIN) — nothing to change."
        exit 0
    fi
fi

# --- Step 3: first-time-only setup — find the vendor file and back it up. ---
if ! $ALREADY_INSTALLED; then
    echo "==> Locating the vendor binfmt.d file that registers ':${ENTRY_NAME}:'..."
    for dir in "${VENDOR_DIRS[@]}"; do
        [[ -d "$dir" ]] || continue
        match="$(grep -rl "^:${ENTRY_NAME}:" "$dir" --include='*.conf' 2>/dev/null | head -n1 || true)"
        if [[ -n "$match" ]]; then
            vendor_file="$match"
            break
        fi
    done
    if [[ -z "$vendor_file" ]]; then
        echo "error: no *.conf under ${VENDOR_DIRS[*]} registers ':${ENTRY_NAME}:'." >&2
        echo "       check 'ls /proc/sys/fs/binfmt_misc/' for the real entry name and pass --entry." >&2
        exit 1
    fi
    basename="$(basename "$vendor_file")"
    OVERRIDE_FILE="$OVERRIDE_DIR/$basename"
    echo "    -> $vendor_file"

    if [[ -e "$OVERRIDE_FILE" ]]; then
        echo "error: $OVERRIDE_FILE already exists but wasn't installed by this script" >&2
        echo "       (no $STATE_FILE). Remove it by hand first if you're sure, or investigate" >&2
        echo "       what put it there — don't want to clobber someone else's override." >&2
        exit 1
    fi

    echo "==> Backing up the original to $BACKUP_DIR (for uninstall's own verification only)..."
    mkdir -p "$BACKUP_DIR"
    cp "$vendor_file" "$ORIGINAL_FILE"
    {
        echo "basename=$basename"
        echo "entry=$ENTRY_NAME"
        echo "vendor_file=$vendor_file"
    } >"$STATE_FILE"
fi

VENDOR_LINE="$(grep "^:${ENTRY_NAME}:" "$ORIGINAL_FILE" | head -n1)"
IFS=':' read -r _ _name type offset magic mask _interp flags <<<"$VENDOR_LINE"

echo "==> Writing $OVERRIDE_FILE (sudo)..."
sudo tee "$OVERRIDE_FILE" >/dev/null <<EOF
# Installed/updated by iprolaunch's scripts/binfmt-override-install.sh.
# Masks $vendor_file (same filename in a higher-precedence binfmt.d
# directory — see binfmt.d(5)) so Windows executables route through
# iprolaunch instead. Revert with scripts/binfmt-override-uninstall.sh.
:${ENTRY_NAME}:${type}:${offset}:${magic}:${mask}:${IPROLAUNCH_BIN}:${flags}
EOF

echo "==> Restarting systemd-binfmt.service (sudo)..."
sudo systemctl restart systemd-binfmt.service

echo "==> Verifying..."
LIVE_INTERPRETER="$(awk '/^interpreter /{print $2}' "/proc/sys/fs/binfmt_misc/${ENTRY_NAME}" 2>/dev/null || true)"
if [[ "$LIVE_INTERPRETER" == "$IPROLAUNCH_BIN" ]]; then
    echo "OK: /proc/sys/fs/binfmt_misc/${ENTRY_NAME} now points at $IPROLAUNCH_BIN"
    echo "Note: the exe itself still needs its +x bit set for the kernel to route it here at all."
else
    echo "WARNING: expected interpreter '$IPROLAUNCH_BIN', got '${LIVE_INTERPRETER:-<none>}'." >&2
    echo "         check: systemctl status systemd-binfmt.service" >&2
    echo "                cat /proc/sys/fs/binfmt_misc/${ENTRY_NAME}" >&2
    exit 1
fi
