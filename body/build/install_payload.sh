#!/usr/bin/env bash
# install_payload.sh — copy the operator, face and session into a body rootfs.
#
# Re-runnable and safe: run it now with placeholders, run it again later once the
# operator/face/session agents have produced their artifacts. It always installs
# SOMETHING valid so the image boots, and upgrades in place when real files exist.
#
#   REPO=/repo ROOTFS=/work/rootfs ./install_payload.sh
#
# Layout it targets (matches the operator/face/session agents' expectations):
#   /opt/brainos/bin/brainos-operator      operator (Rust) or placeholder
#   /opt/brainos/face/brainos-face         face entrypoint (+ brainos_face/ pkg)
#   /etc/brainos/session/brainos-session   session launcher (+ hyprland/sway/… conf)
#   /home/human/.bash_profile              execs the session on tty1
#   /etc/brainos/operator.toml             operator config
#
# Runs as root inside the build container, but works anywhere the paths resolve.
set -uo pipefail

REPO="${REPO:-$(cd "$(dirname "$0")/../.." && pwd)}"
ROOTFS="${ROOTFS:?set ROOTFS to the body rootfs path}"
CFG="$REPO/body/build/config"
say(){ printf '  payload: %s\n' "$*"; }

install -d -m0755 "$ROOTFS/opt/brainos/bin" \
                  "$ROOTFS/opt/brainos/face" \
                  "$ROOTFS/etc/brainos/session" \
                  "$ROOTFS/opt/brainos/models" \
                  "$ROOTFS/opt/brainos/voices" \
                  "$ROOTFS/etc/brainos"

# ---- operator (Rust) -------------------------------------------------------
OP="$REPO/body/operator/target/release/brainos-operator"
if [ -x "$OP" ]; then
  install -m0755 "$OP" "$ROOTFS/opt/brainos/bin/brainos-operator"; say "operator: real binary"
else
  cat > "$ROOTFS/opt/brainos/bin/brainos-operator" <<'PH'
#!/usr/bin/env bash
# PLACEHOLDER operator — the Rust operator binary is not built yet. Creates the
# IPC socket dir the face expects, then idles so the session slot is filled.
sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/brainos"
mkdir -p "$sock"
echo "brainos-operator placeholder: no operator binary installed yet" >&2
exec sleep infinity
PH
  chmod +x "$ROOTFS/opt/brainos/bin/brainos-operator"; say "operator: placeholder"
fi

# ---- face (Python GTK) -----------------------------------------------------
if [ -x "$REPO/body/face/brainos-face" ]; then
  rm -rf "$ROOTFS/opt/brainos/face"; install -d -m0755 "$ROOTFS/opt/brainos/face"
  cp -a "$REPO/body/face/brainos-face" "$ROOTFS/opt/brainos/face/brainos-face"
  [ -d "$REPO/body/face/brainos_face" ] && cp -a "$REPO/body/face/brainos_face" "$ROOTFS/opt/brainos/face/"
  chmod +x "$ROOTFS/opt/brainos/face/brainos-face"
  say "face: installed from body/face"
elif [ ! -e "$ROOTFS/opt/brainos/face/brainos-face" ]; then
  cat > "$ROOTFS/opt/brainos/face/brainos-face" <<'PH'
#!/usr/bin/env bash
# PLACEHOLDER face — the GTK face is not installed yet. Remote-control flags
# (--key-down/--key-up/--cancel/…) are accepted and ignored so binds don't error.
case "${1:-}" in --*) exit 0;; esac
echo "brainos-face placeholder: no face installed yet" >&2
exec sleep infinity
PH
  chmod +x "$ROOTFS/opt/brainos/face/brainos-face"; say "face: placeholder"
fi

# ---- session ---------------------------------------------------------------
if [ -x "$REPO/body/session/brainos-session" ]; then
  # Install the whole session config tree to /etc/brainos/session (its CONF root).
  for f in "$REPO/body/session"/*; do
    bn="$(basename "$f")"; [ "$bn" = "bash_profile" ] && continue
    cp -a "$f" "$ROOTFS/etc/brainos/session/"
  done
  chmod +x "$ROOTFS/etc/brainos/session/brainos-session"
  say "session: installed from body/session"
elif [ ! -e "$ROOTFS/etc/brainos/session/brainos-session" ]; then
  install -m0755 "$CFG/brainos-session.placeholder" "$ROOTFS/etc/brainos/session/brainos-session"
  install -m0644 "$CFG/hyprland.conf.placeholder"   "$ROOTFS/etc/brainos/session/hyprland.conf"
  say "session: placeholder"
fi
# convenience symlink so /opt/brainos/bin/brainos-session also resolves
ln -sf /etc/brainos/session/brainos-session "$ROOTFS/opt/brainos/bin/brainos-session"
# the face lives at /opt/brainos/face/brainos-face (hyprland execs it there); drop
# any stale bin launcher from an earlier layout so there is one source of truth
[ -f "$ROOTFS/opt/brainos/bin/brainos-face" ] && rm -f "$ROOTFS/opt/brainos/bin/brainos-face" || true

# ---- bash_profile (execs the session on tty1) ------------------------------
if [ -f "$REPO/body/session/bash_profile" ]; then
  install -m0644 "$REPO/body/session/bash_profile" "$ROOTFS/home/human/.bash_profile"; say "bash_profile: from body/session"
else
  install -m0644 "$CFG/bash_profile" "$ROOTFS/home/human/.bash_profile"; say "bash_profile: default"
fi

# ---- operator config (always refresh from repo) ----------------------------
install -m0644 "$CFG/operator.toml" "$ROOTFS/etc/brainos/operator.toml"

echo "payload install complete."
