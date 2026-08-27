#!/usr/bin/env bash
# build_in_container.sh — runs as root inside a privileged archlinux container.
# Produces work/out/body.ext4 (p2 rootfs) and work/out/BODY.EFI (the body UKI).
#
# Mounts expected (set up by build_body.sh):
#   /repo                        -> the brainos git repo (ro in spirit)
#   /work                        -> body/build/work   (rw, persistent)
#   /var/cache/pacman/pkg        -> work/cache/pacman (persistent pkg cache)
# Env:
#   WITH_MODELS=1|0   WITH_DEV_SSH=1|0   FORCE_ROOTFS=1|0   LLAMA_VULKAN=1|0
set -euo pipefail

REPO=/repo
WORK=/work
CFG="$REPO/body/build/config"
ROOTFS="$WORK/rootfs"
OUT="$WORK/out"
CACHE="$WORK/cache"
WITH_MODELS="${WITH_MODELS:-1}"
WITH_DEV_SSH="${WITH_DEV_SSH:-1}"
FORCE_ROOTFS="${FORCE_ROOTFS:-0}"
LLAMA_VULKAN="${LLAMA_VULKAN:-1}"

mkdir -p "$OUT" "$CACHE/models" "$CACHE/src" "$CACHE/bin"
step(){ printf '\n\033[1;36m==== %s ====\033[0m\n' "$*"; }

# ---------------------------------------------------------------------------
step "container prep: keyring + build tools"
pacman -Sy --noconfirm --needed arch-install-scripts git cmake ninja pkgconf \
    vulkan-headers vulkan-icd-loader shaderc glslang spirv-tools spirv-headers \
    openssh curl >/dev/null
# host keyring (used to verify packages pacstrap pulls)
if [ ! -s /etc/pacman.d/gnupg/pubring.gpg ]; then
  pacman-key --init
  pacman-key --populate archlinux
fi

# ---------------------------------------------------------------------------
PKGS=(
  base linux linux-firmware
  linux-firmware-intel linux-firmware-realtek linux-firmware-atheros
  linux-firmware-broadcom linux-firmware-mediatek linux-firmware-nvidia
  linux-firmware-amdgpu
  intel-ucode amd-ucode
  mkinitcpio systemd-ukify efibootmgr
  networkmanager iwd bluez bluez-utils
  pipewire pipewire-pulse wireplumber
  mesa vulkan-intel vulkan-radeon vulkan-nouveau vulkan-icd-loader
  hyprland xdg-desktop-portal-hyprland hyprpaper hyprlock
  sway swaybg foot chromium
  grim slurp wtype ydotool wl-clipboard cliphist brightnessctl playerctl
  python python-gobject python-cairo python-pillow
  gtk4 gtk4-layer-shell webkitgtk-6.0 cairo pango glib2
  nodejs git openssh htop
  noto-fonts noto-fonts-emoji ttf-jetbrains-mono ttf-jetbrains-mono-nerd inter-font terminus-font
  xdg-utils polkit seatd sudo nano vim less ripgrep jq curl wget unzip
  dosfstools e2fsprogs mtools usbutils pciutils upower power-profiles-daemon
  cloud-guest-utils gptfdisk parted gnome-text-editor
)

if [ "$FORCE_ROOTFS" = "1" ]; then rm -rf "$ROOTFS"; fi

if [ ! -e "$ROOTFS/usr/bin/bash" ]; then
  step "pacstrap ${#PKGS[@]} package groups into rootfs"
  mkdir -p "$ROOTFS"
  # -c: use the (bind-mounted) host package cache so rebuilds are fast
  # -G: don't copy host keyring perms; -M: don't copy host mirrorlist (use image's)
  pacstrap -c -G "$ROOTFS" "${PKGS[@]}"
else
  step "rootfs exists — syncing package set (idempotent)"
  cp -a /etc/pacman.d/mirrorlist "$ROOTFS/etc/pacman.d/mirrorlist" 2>/dev/null || true
  pacstrap -c -G "$ROOTFS" "${PKGS[@]}"
fi

# ---------------------------------------------------------------------------
step "place configuration files"
install -Dm0644 "$CFG/mkinitcpio.conf"   "$ROOTFS/etc/mkinitcpio.conf"
install -Dm0644 "$CFG/fstab"             "$ROOTFS/etc/fstab"
install -Dm0644 "$CFG/os-release"        "$ROOTFS/etc/os-release"
install -Dm0644 "$CFG/getty-autologin.conf" "$ROOTFS/etc/systemd/system/getty@tty1.service.d/autologin.conf"
install -Dm0644 "$CFG/brainos-llama.service"    "$ROOTFS/etc/systemd/system/brainos-llama.service"
install -Dm0644 "$CFG/ydotoold.service"         "$ROOTFS/etc/systemd/system/ydotoold.service"
install -Dm0644 "$CFG/brainos-firstboot.service" "$ROOTFS/etc/systemd/system/brainos-firstboot.service"
install -Dm0755 "$CFG/brainos-firstboot"        "$ROOTFS/opt/brainos/bin/brainos-firstboot"
install -Dm0644 "$CFG/brainos-tmpfiles.conf"    "$ROOTFS/etc/tmpfiles.d/brainos.conf"
install -Dm0644 "$CFG/90-brainos-secure.rules"      "$ROOTFS/etc/udev/rules.d/90-brainos-secure.rules"
install -Dm0440 "$CFG/sudoers-human"            "$ROOTFS/etc/sudoers.d/10-human"
install -Dm0644 "$CFG/human-environment"        "$ROOTFS/etc/environment.d/brainos.conf"
install -Dm0644 "$CFG/bash_profile"             "$ROOTFS/home/human/.bash_profile"
echo "brainos" > "$ROOTFS/etc/hostname"
cat > "$ROOTFS/etc/hosts" <<HOSTS
127.0.0.1   localhost
::1         localhost
127.0.1.1   brainos.localdomain brainos
HOSTS
echo "LANG=en_US.UTF-8" > "$ROOTFS/etc/locale.conf"
echo "KEYMAP=us" > "$ROOTFS/etc/vconsole.conf"
# NetworkManager: use iwd as the wifi backend
install -Dm0644 /dev/stdin "$ROOTFS/etc/NetworkManager/conf.d/wifi_backend.conf" <<NMEOF
[device]
wifi.backend=iwd
NMEOF

# ---------------------------------------------------------------------------
step "build llama.cpp"
build_llama() {
  local src="$CACHE/src/llama.cpp"
  if [ ! -d "$src/.git" ]; then
    git clone --depth 1 https://github.com/ggml-org/llama.cpp "$src"
  else
    git -C "$src" pull --ff-only || true
  fi
  local vk=OFF; [ "$LLAMA_VULKAN" = "1" ] && vk=ON
  rm -rf "$src/build"
  if ! cmake -S "$src" -B "$src/build" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_SHARED_LIBS=OFF \
        -DLLAMA_CURL=OFF \
        -DGGML_NATIVE=OFF \
        -DGGML_VULKAN=$vk >/dev/null 2>"$CACHE/src/llama_cmake.err"; then
      echo "!! Vulkan cmake failed; retrying CPU-only"; cat "$CACHE/src/llama_cmake.err" | tail -5
      rm -rf "$src/build"
      cmake -S "$src" -B "$src/build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_SHARED_LIBS=OFF -DLLAMA_CURL=OFF -DGGML_NATIVE=OFF -DGGML_VULKAN=OFF >/dev/null
  fi
  cmake --build "$src/build" -j"$(nproc)" --target llama-server llama-cli >/dev/null
  cp "$src/build/bin/llama-server" "$CACHE/bin/llama-server"
  cp "$src/build/bin/llama-cli"    "$CACHE/bin/llama-cli"
}
if [ ! -x "$CACHE/bin/llama-server" ]; then build_llama; else echo "llama-server cached"; fi
install -Dm0755 "$CACHE/bin/llama-server" "$ROOTFS/usr/bin/llama-server"
install -Dm0755 "$CACHE/bin/llama-cli"    "$ROOTFS/usr/bin/llama-cli"

step "build whisper.cpp"
build_whisper() {
  local src="$CACHE/src/whisper.cpp"
  if [ ! -d "$src/.git" ]; then
    git clone --depth 1 https://github.com/ggml-org/whisper.cpp "$src"
  else
    git -C "$src" pull --ff-only || true
  fi
  rm -rf "$src/build"
  cmake -S "$src" -B "$src/build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_EXAMPLES=ON -DGGML_NATIVE=OFF >/dev/null
  cmake --build "$src/build" -j"$(nproc)" --target whisper-cli >/dev/null
  cp "$src/build/bin/whisper-cli" "$CACHE/bin/whisper-cli"
}
if [ ! -x "$CACHE/bin/whisper-cli" ]; then build_whisper; else echo "whisper-cli cached"; fi
install -Dm0755 "$CACHE/bin/whisper-cli" "$ROOTFS/usr/bin/whisper-cli"

step "vendor piper"
if [ -d "$REPO/tools/piper/piper" ]; then
  mkdir -p "$ROOTFS/opt/brainos/piper"
  cp -a "$REPO/tools/piper/piper/." "$ROOTFS/opt/brainos/piper/"
  chmod +x "$ROOTFS/opt/brainos/piper/piper" 2>/dev/null || true
  echo "piper vendored from tools/piper"
else
  echo "!! no vendored piper found"
fi

# ---------------------------------------------------------------------------
step "models"
mkdir -p "$ROOTFS/opt/brainos/models" "$ROOTFS/opt/brainos/voices"
# piper voices: copy the ones already on the host (best available)
for v in en_US-lessac-medium en_US-ryan-high; do
  if [ -f "$REPO/tools/piper/$v.onnx" ]; then
    cp -a "$REPO/tools/piper/$v.onnx" "$REPO/tools/piper/$v.onnx.json" "$ROOTFS/opt/brainos/voices/" 2>/dev/null || true
  fi
done
fetch(){ # url dest
  local url="$1" dst="$2" cf="$CACHE/models/$(basename "$2")"
  if [ ! -s "$cf" ]; then
    echo "downloading $(basename "$dst") ..."
    curl -fL --retry 3 --retry-delay 2 -o "$cf.part" "$url" && mv "$cf.part" "$cf"
  else echo "$(basename "$dst") cached"; fi
  cp -a "$cf" "$dst"
}
if [ "$WITH_MODELS" = "1" ]; then
  fetch "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf" \
        "$ROOTFS/opt/brainos/models/qwen2.5-1.5b-instruct-q4_k_m.gguf"
  fetch "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin" \
        "$ROOTFS/opt/brainos/models/ggml-base.en.bin"
else
  echo "WITH_MODELS=0 — skipping model downloads"
fi

# ---------------------------------------------------------------------------
step "install operator/face/session payload"
REPO="$REPO" ROOTFS="$ROOTFS" "$REPO/body/build/install_payload.sh"

# ---------------------------------------------------------------------------
step "dev ssh"
if [ "$WITH_DEV_SSH" = "1" ]; then
  install -Dm0644 "$CFG/sshd_dev.conf" "$ROOTFS/etc/ssh/sshd_config.d/10-brainos-dev.conf"
  # generate a dev keypair into /work/ssh if absent
  if [ ! -f "$WORK/ssh/brainos_dev_ed25519" ]; then
    mkdir -p "$WORK/ssh"
    ssh-keygen -t ed25519 -N '' -C 'brainos-dev-harness' -f "$WORK/ssh/brainos_dev_ed25519" >/dev/null
  fi
  # The harness runs unprivileged as uid 1000 on the host; hand it the key.
  chown 1000:1000 "$WORK/ssh/brainos_dev_ed25519" "$WORK/ssh/brainos_dev_ed25519.pub" 2>/dev/null || true
  chmod 600 "$WORK/ssh/brainos_dev_ed25519"; chmod 644 "$WORK/ssh/brainos_dev_ed25519.pub"
  install -d -m0700 "$ROOTFS/home/human/.ssh"
  install -m0600 "$WORK/ssh/brainos_dev_ed25519.pub" "$ROOTFS/home/human/.ssh/authorized_keys"
  echo "dev ssh key installed (work/ssh/brainos_dev_ed25519)"
else
  rm -f "$ROOTFS/etc/ssh/sshd_config.d/10-brainos-dev.conf"
  rm -f "$ROOTFS/home/human/.ssh/authorized_keys"
  echo "production build: no dev ssh"
fi

# ---------------------------------------------------------------------------
step "chroot configuration (locale, user, services, initramfs, UKI)"
install -Dm0755 "$CFG/configure_rootfs.sh" "$ROOTFS/root/configure_rootfs.sh"
arch-chroot "$ROOTFS" /root/configure_rootfs.sh
rm -f "$ROOTFS/root/configure_rootfs.sh"
# enable sshd only in dev builds (after chroot base config)
if [ "$WITH_DEV_SSH" = "1" ]; then
  arch-chroot "$ROOTFS" systemctl enable sshd.service
fi
chown -R 1000:1000 "$ROOTFS/home/human"

# ---------------------------------------------------------------------------
step "export BODY.EFI"
cp "$ROOTFS/boot/BODY.EFI" "$OUT/BODY.EFI"
echo "BODY.EFI -> $OUT/BODY.EFI ($(stat -c%s "$OUT/BODY.EFI") bytes)"

# ---------------------------------------------------------------------------
step "mkfs.ext4 body.ext4 (contents + 15% headroom)"
# The kernel + initramfs live in the UKI on p1; the /boot copies inside rootfs
# are redundant for booting. Keep them (harmless, useful for rebuilds).
USED_KIB=$(du -sk "$ROOTFS" | cut -f1)
SIZE_KIB=$(( USED_KIB * 115 / 100 + 262144 ))   # +15% and +256MiB slack
SIZE_M=$(( (SIZE_KIB + 1023) / 1024 ))
echo "rootfs used ${USED_KIB} KiB -> image ${SIZE_M} MiB"
rm -f "$OUT/body.ext4"
mkfs.ext4 -q -L BRAINOS-BODY -E lazy_itable_init=0,lazy_journal_init=0 \
  -d "$ROOTFS" "$OUT/body.ext4" "${SIZE_M}M"
echo "body.ext4 -> $OUT/body.ext4 ($(du -h "$OUT/body.ext4" | cut -f1))"

# ---------------------------------------------------------------------------
step "PACKAGES.txt"
arch-chroot "$ROOTFS" pacman -Qe > "$WORK/PACKAGES.txt" 2>/dev/null || \
  pacman -r "$ROOTFS" -Qe > "$WORK/PACKAGES.txt"
echo "wrote $WORK/PACKAGES.txt ($(wc -l < "$WORK/PACKAGES.txt") explicit packages)"

step "DONE"
echo "outputs in $OUT:"; ls -la "$OUT"
