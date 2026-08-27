#!/usr/bin/env bash
# Runs INSIDE the target via arch-chroot. Only actions that need the target's own
# tools live here; plain file placement is done from the container side.
set -euo pipefail

# locale / time / host
sed -i 's/^#\(en_US.UTF-8 UTF-8\)/\1/' /etc/locale.gen
locale-gen
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
hwclock --systohc 2>/dev/null || true

# user `human` (uid 1000), groups, NO password, passwordless sudo
groupadd -f -g 1000 human
id human >/dev/null 2>&1 || useradd -m -u 1000 -g human \
  -G wheel,video,input,audio,seat,network,storage,render -s /bin/bash human
passwd -d human || true          # delete password (empty, but locked-open)
# ensure home perms
chown -R 1000:1000 /home/human

# enable services
systemctl enable NetworkManager.service
systemctl enable bluetooth.service
systemctl enable systemd-timesyncd.service
systemctl enable ydotoold.service
systemctl enable brainos-llama.service
systemctl enable brainos-firstboot.service
systemctl enable seatd.service 2>/dev/null || true
systemctl enable power-profiles-daemon.service 2>/dev/null || true
# NM should use iwd as the wifi backend
systemctl enable iwd.service 2>/dev/null || true
# getty tty1 is enabled by default target; ensure it
systemctl enable getty@tty1.service 2>/dev/null || true

# machine-id: empty so each physical stick regenerates a unique one on first boot
: > /etc/machine-id

# regenerate initramfs (universal, no autodetect) then build the UKI
KVER="$(basename "$(ls -d /usr/lib/modules/*/ | head -1)")"
echo "chroot: kernel $KVER"
mkinitcpio -g /boot/initramfs-linux.img -k "$KVER"

ukify build \
  --linux=/boot/vmlinuz-linux \
  --initrd=/boot/intel-ucode.img \
  --initrd=/boot/amd-ucode.img \
  --initrd=/boot/initramfs-linux.img \
  --cmdline="root=PARTLABEL=BRAINOS-BODY rw rootwait quiet splash loglevel=3 systemd.show_status=0 mitigations=auto console=tty1 console=ttyS0" \
  --os-release=@/etc/os-release \
  --output=/boot/BODY.EFI
echo "chroot: BODY.EFI built ($(stat -c%s /boot/BODY.EFI) bytes)"
