#!/bin/bash
# Build a golden station image by chroot-customizing a stock Raspberry Pi OS
# release. Produces dist/beamer-<date>-<target>.img.xz, ready to flash to every
# card in the fleet with no per-unit editing.
#
#   sudo ./build-image.sh armhf        Pi Zero W
#
# Runs as root on a Linux host with loop devices - the lima VM from
# ../vm/mac-build.sh, or any Linux box. See README.md.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
CACHE=${CACHE:-$REPO/.cache}
DIST=${DIST:-$REPO/dist}
XZ_LEVEL=${XZ_LEVEL:-6}
[[ "$XZ_LEVEL" =~ ^[0-9]$ ]] || { echo "ERROR: XZ_LEVEL must be 0-9, got '$XZ_LEVEL'" >&2; exit 1; }

TARGET=${1:-}
[[ -n "$TARGET" ]] || { echo "usage: $0 <armhf>" >&2; exit 1; }
CONF="$HERE/targets/$TARGET.conf"
[[ -f "$CONF" ]] || { echo "ERROR: no such target: $TARGET" >&2; exit 1; }
[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }

[[ -n "${BEAMER_USER_PASS:-}" ]] || {
    echo "ERROR: set BEAMER_USER_PASS." >&2
    echo "This image bakes its own login; Imager's user/SSH fields no longer" >&2
    echo "apply, so without it the station is unreachable." >&2
    exit 1
}

source "$CONF"

for t in losetup sfdisk resize2fs e2fsck dumpe2fs mkfs.vfat mmd mcopy mdir xz curl sha256sum readelf python3; do
    command -v "$t" >/dev/null || { echo "ERROR: missing tool: $t" >&2; exit 1; }
done

WORK=$(mktemp -d)
ROOTFS="$WORK/rootfs"
LOOP=
mkdir -p "$ROOTFS" "$CACHE" "$DIST"

cleanup() {
    set +e
    umount -R "$ROOTFS" 2>/dev/null
    [[ -n "$LOOP" ]] && losetup -d "$LOOP" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

say() { echo; echo "==> $*"; }

# --- 1. base image --------------------------------------------------------
XZ_NAME=$(basename "$BASE_IMAGE_URL")
XZ_PATH="$CACHE/$XZ_NAME"
if [[ ! -f "$XZ_PATH" ]]; then
    say "downloading $XZ_NAME"
    curl -fL --progress-bar -o "$XZ_PATH.part" "$BASE_IMAGE_URL"
    mv "$XZ_PATH.part" "$XZ_PATH"
fi

say "verifying checksum"
echo "$BASE_IMAGE_SHA256  $XZ_PATH" | sha256sum -c - || {
    echo "ERROR: checksum mismatch. Refusing to build on an unverified base." >&2
    echo "Delete $XZ_PATH and retry, or update the hash in $CONF." >&2
    exit 1
}

IMG="$WORK/work.img"
say "decompressing"
xz -dc "$XZ_PATH" > "$IMG"

# --- 2. grow the rootfs ---------------------------------------------------
say "growing rootfs by ${GROW_MB}MB"
truncate -s "+${GROW_MB}M" "$IMG"
sfdisk --no-reread -N 2 "$IMG" <<<", +" >/dev/null

LOOP=$(losetup -fP --show "$IMG")
[[ -e "${LOOP}p2" ]] || { echo "ERROR: partition scan failed" >&2; exit 1; }
e2fsck -fy "${LOOP}p2" || true
resize2fs "${LOOP}p2"

# --- 3. mount -------------------------------------------------------------
say "mounting"
mount "${LOOP}p2" "$ROOTFS"
mkdir -p "$ROOTFS/boot/firmware"
mount "${LOOP}p1" "$ROOTFS/boot/firmware"

ELF_ARCH=$(readelf -h "$ROOTFS/bin/true" | sed -n 's/^ *Machine: *//p')
[[ "$ELF_ARCH" == *"$EXPECT_ELF_ARCH"* ]] || {
    echo "ERROR: target $TARGET expects $EXPECT_ELF_ARCH, image contains '$ELF_ARCH'" >&2
    echo "BASE_IMAGE_URL in $CONF is probably pointing at the wrong architecture." >&2
    exit 1
}

# --- 4. chroot plumbing ---------------------------------------------------
say "preparing chroot"
mount --bind /dev     "$ROOTFS/dev"
mount --bind /dev/pts "$ROOTFS/dev/pts"
mount -t proc  proc   "$ROOTFS/proc"
mount -t sysfs sysfs  "$ROOTFS/sys"
cp /etc/resolv.conf "$ROOTFS/etc/resolv.conf"

cat > "$ROOTFS/usr/sbin/policy-rc.d" <<'EOF'
#!/bin/sh
exit 101
EOF
chmod 0755 "$ROOTFS/usr/sbin/policy-rc.d"

if [[ -n "$QEMU_STATIC" ]]; then
    [[ -x "$QEMU_STATIC" ]] || {
        echo "ERROR: $QEMU_STATIC not found." >&2
        echo "Install it on the build host: apt-get install qemu-user-static binfmt-support" >&2
        exit 1
    }
    [[ -e /proc/sys/fs/binfmt_misc/qemu-arm ]] || {
        echo "ERROR: qemu-arm is not registered in binfmt_misc on this host." >&2
        echo "Note that installing qemu-user-static is not sufficient on an" >&2
        echo "arm64 builder - it ships the interpreter but omits the" >&2
        echo "registration. See the registration block in ../vm/mac-build.sh." >&2
        exit 1
    }
    cp "$QEMU_STATIC" "$ROOTFS/usr/bin/"
fi

# --- 5. bake --------------------------------------------------------------
say "baking"
mkdir -p "$ROOTFS/tmp/beamer-build"
cp -r "$REPO/scripts" "$HERE/bake.sh" "$ROOTFS/tmp/beamer-build/"
chroot "$ROOTFS" env SCRIPTS=/tmp/beamer-build/scripts \
    BEAMER_USER="${BEAMER_USER:-beamer}" \
    BEAMER_USER_PASS="$BEAMER_USER_PASS" \
    /bin/bash /tmp/beamer-build/bake.sh

# --- 6. template image ----------------------------------------------------
say "building template image"
"$REPO/scripts/make-fs.sh" "$WORK/gadget-template.img"

TEMPLATE_IMG="$WORK/gadget-template.img@@1048576"
for d in ::/CONFIG ::/SLIPPI; do
    mdir -b -i "$TEMPLATE_IMG" "$d" >/dev/null 2>&1 || {
        echo "ERROR: make-fs.sh did not create $d in the template" >&2
        exit 1
    }
done
mdir -b -i "$TEMPLATE_IMG" ::/CONFIG/config.txt >/dev/null 2>&1 || {
    echo "ERROR: make-fs.sh did not create ::/CONFIG/config.txt in the template" >&2
    echo "Stations would have nowhere to read their settings from." >&2
    exit 1
}

cp --sparse=always "$WORK/gadget-template.img" "$ROOTFS/srv/gadget-template.img"

# --- 7. boot partition ----------------------------------------------------
say "configuring boot partition"
BOOT="$ROOTFS/boot/firmware"

assert_config_section() {
    local want=$1 section
    section=$(awk -v want="$want" '
        /^\[.*\]/ { section = $0 }
        $0 == want { print (section == "" ? "[none]" : section) }
    ' "$BOOT/config.txt" | tail -n1)
    [[ "$section" == "[all]" || "$section" == "[none]" ]] || {
        echo "ERROR: $want landed under $section, not [all]" >&2
        echo "It would be silently ignored on the Zero W and Zero 2 W." >&2
        exit 1
    }
}

append_config() {
    local line=$1 last
    if grep -qxF -- "$line" "$BOOT/config.txt"; then
        return 0
    fi
    last=$(awk '/^\[.*\]/ { s = $0 } END { print s }' "$BOOT/config.txt")
    if [[ "$last" != "[all]" ]]; then
        printf '\n[all]\n' >> "$BOOT/config.txt"
    fi
    printf '%s\n' "$line" >> "$BOOT/config.txt"
}

disable_config() {
    local line=$1
    grep -qxF -- "$line" "$BOOT/config.txt" || {
        echo "ERROR: expected '$line' in the stock config.txt; it is not there." >&2
        echo "The pinned base image has changed shape. Re-read it and update" >&2
        echo "this list, or drop the line if the stock image no longer sets it." >&2
        exit 1
    }
    awk -v want="$line" '
        $0 == want { print "# " $0 "  (disabled by the beamer build)"; next }
        { print }
    ' "$BOOT/config.txt" > "$BOOT/config.txt.new"
    mv -f "$BOOT/config.txt.new" "$BOOT/config.txt"
}

disable_config 'dtparam=audio=on'
disable_config 'camera_auto_detect=1'
disable_config 'display_auto_detect=1'
disable_config 'dtoverlay=vc4-kms-v3d'
disable_config 'max_framebuffers=2'

# Measured on a Zero W: the firmware reads the SD at about 1.7 MB/s, so the
# 13.9 MB initramfs cost 8.1 seconds of pre-kernel time - by far the largest
# single item in the boot. Raspberry Pi kernels build in the MMC and ext4
# drivers and resolve root=PARTUUID= natively, so nothing here needs one. What
# it did do was grow the rootfs (the 'resize' cmdline token, dropped below) and
# fsck root; those become beamer-growfs.service and systemd-fsck-root.
disable_config 'auto_initramfs=1'

append_config 'gpu_mem=16'
assert_config_section 'gpu_mem=16'

append_config 'dtoverlay=dwc2,dr_mode=peripheral'
assert_config_section 'dtoverlay=dwc2,dr_mode=peripheral'

append_config 'dtparam=act_led_trigger=timer'
assert_config_section 'dtparam=act_led_trigger=timer'

append_config 'disable_splash=1'
assert_config_section 'disable_splash=1'

append_config 'boot_delay=0'
assert_config_section 'boot_delay=0'

append_config 'dtoverlay=disable-bt'
assert_config_section 'dtoverlay=disable-bt'

append_config 'initial_turbo=30'
assert_config_section 'initial_turbo=30'

# --- cmdline.txt ----------------------------------------------------------
CMDLINE_BEFORE=$(cat "$BOOT/cmdline.txt")
CMDLINE_DROP=('console=serial0,115200' 'resize')
CMDLINE_ADD=('quiet' 'loglevel=3' 'modules-load=dwc2,libcomposite')

CMDLINE_NEW=()
for tok in $CMDLINE_BEFORE; do
    drop=0
    for d in "${CMDLINE_DROP[@]}"; do
        [[ "$tok" == "$d" ]] && drop=1
    done
    [[ "$tok" == modules-load=* ]] && drop=1
    (( drop )) || CMDLINE_NEW+=("$tok")
done
for a in "${CMDLINE_ADD[@]}"; do
    present=0
    for tok in "${CMDLINE_NEW[@]}"; do
        [[ "$tok" == "$a" ]] && present=1
    done
    (( present )) || CMDLINE_NEW+=("$a")
done
printf '%s\n' "${CMDLINE_NEW[*]}" > "$BOOT/cmdline.txt"

[[ $(wc -l < "$BOOT/cmdline.txt") -le 1 ]] || {
    echo "ERROR: cmdline.txt gained a second line; it must stay one line" >&2
    exit 1
}

for tok in $CMDLINE_BEFORE; do
    dropped=0
    for d in "${CMDLINE_DROP[@]}"; do
        [[ "$tok" == "$d" ]] && dropped=1
    done
    [[ "$tok" == modules-load=* ]] && dropped=1
    (( dropped )) && continue
    grep -qw -- "$tok" "$BOOT/cmdline.txt" || {
        echo "ERROR: our cmdline.txt edit dropped the stock token '$tok'" >&2
        echo "before: $CMDLINE_BEFORE" >&2
        echo "after:  $(cat "$BOOT/cmdline.txt")" >&2
        exit 1
    }
done

# --- cloud-init seed ------------------------------------------------------
rm -f "$BOOT/user-data" "$BOOT/meta-data" "$BOOT/network-config"

# --- initramfs ------------------------------------------------------------
rm -f "$BOOT"/initramfs*
shopt -s nullglob
INITRAMFS_LEFT=("$BOOT"/initramfs*)
shopt -u nullglob
(( ${#INITRAMFS_LEFT[@]} == 0 )) || {
    echo "ERROR: initramfs files survived deletion: ${INITRAMFS_LEFT[*]}" >&2
    exit 1
}

# --- 8. strip per-machine state -------------------------------------------
say "clearing per-machine state"
[[ -n "$QEMU_STATIC" ]] && rm -f "$ROOTFS/usr/bin/$(basename "$QEMU_STATIC")"
rm -f "$ROOTFS/usr/sbin/policy-rc.d"
printf 'uninitialized\n' > "$ROOTFS/etc/machine-id" # "uninitialized", NOT an empty file. 
rm -f "$ROOTFS/var/lib/dbus/machine-id"
rm -f "$ROOTFS/etc/ssh/ssh_host_"*
rm -rf "$ROOTFS/var/lib/apt/lists/"*
rm -rf "$ROOTFS/var/cache/apt/archives/"*.deb
find "$ROOTFS/var/log" -type f -delete
: > "$ROOTFS/etc/resolv.conf"
rm -rf "$ROOTFS/var/lib/beamer" "$ROOTFS/srv/gadget.img"

[[ "$(cat "$ROOTFS/etc/machine-id")" == "uninitialized" ]] || {
    echo "ERROR: /etc/machine-id must contain 'uninitialized', not '$(cat "$ROOTFS/etc/machine-id")'" >&2
    echo "An empty file disables ConditionFirstBoot=yes units - see machine-id(5)." >&2
    exit 1
}
[[ -L "$ROOTFS/etc/systemd/system/sysinit.target.wants/regenerate_ssh_host_keys.service" ]] || {
    echo "ERROR: regenerate_ssh_host_keys.service is not enabled in the image." >&2
    echo "Host keys were just deleted and nothing else regenerates them." >&2
    echo "SSH would never come up." >&2
    exit 1
}
[[ -L "$ROOTFS/etc/systemd/system/multi-user.target.wants/ssh.service" ]] || {
    echo "ERROR: ssh.service is not enabled in the image (see bake.sh)." >&2
    echo "A station has no console to debug from; SSH is the only way in." >&2
    exit 1
}
BEAMER_SHADOW=$(sed -n "s/^${BEAMER_USER:-beamer}://p" "$ROOTFS/etc/shadow" | cut -d: -f1)
[[ "$BEAMER_SHADOW" == \$* ]] || {
    echo "ERROR: ${BEAMER_USER:-beamer} has no password hash in /etc/shadow" >&2
    echo "(found '${BEAMER_SHADOW:-nothing}'). bake.sh did not bake a login." >&2
    exit 1
}
[[ -s "$ROOTFS/etc/sudoers.d/010-beamer" ]] || {
    echo "ERROR: bake.sh did not write the sudoers drop-in; sudo would need a" >&2
    echo "password prompt that nothing on a headless station can answer." >&2
    exit 1
}
[[ -x "$ROOTFS/usr/local/lib/beamer/cgi/beamer-api.cgi" ]] || {
    echo "ERROR: beamer-api.cgi is missing or not executable." >&2
    echo "/status and /reset-beamer would 500 on every request." >&2
    exit 1
}
[[ -s "$ROOTFS/etc/polkit-1/rules.d/50-beamer-web.rules" ]] || {
    echo "ERROR: bake.sh did not install the polkit rule. The CGI could not" >&2
    echo "start the status check or the reset, and both POST endpoints would fail" >&2
    echo "- POST /status by handing back a stale report." >&2
    exit 1
}
grep -q 'status-check.service' "$ROOTFS/etc/polkit-1/rules.d/50-beamer-web.rules" || {
    echo "ERROR: the polkit rule does not whitelist status-check.service, so" >&2
    echo "POST /status would be denied and serve a stale report forever." >&2
    exit 1
}
[[ -x "$ROOTFS/usr/local/lib/beamer/slp-peek" ]] || {
    echo "ERROR: slp-peek did not compile during bake. Without it nothing can" >&2
    echo "tell a finished replay from one still being written, and the station" >&2
    echo "would publish files replay-manager cannot parse." >&2
    exit 1
}
[[ ! -e "$ROOTFS/usr/bin/gcc" ]] || {
    echo "ERROR: gcc is still installed; the slp-peek block in bake.sh should" >&2
    echo "have purged it. It is pure weight in every image from here on." >&2
    exit 1
}
for u in status-check.timer health-check.timer health-check.service; do
    [[ -s "$ROOTFS/etc/systemd/system/$u" ]] || {
        echo "ERROR: $u is missing; nothing would publish replays or check health." >&2
        exit 1
    }
done
[[ ! -e "$ROOTFS/etc/systemd/system/flush-gadget-data.timer" ]] || {
    echo "ERROR: flush-gadget-data.timer is still installed. status-check now" >&2
    echo "drives the flush; leaving the old timer would publish in-progress games." >&2
    exit 1
}
[[ -s "$ROOTFS/etc/systemd/system/beamer-reset.service" ]] || {
    echo "ERROR: beamer-reset.service is missing; POST /reset-beamer has" >&2
    echo "nothing to start." >&2
    exit 1
}
[[ -s "$ROOTFS/var/www/html/SLIPPI/index.json" ]] || {
    echo "ERROR: the seed replay index is missing; GET /SLIPPI/ would 404 until" >&2
    echo "the first flush." >&2
    exit 1
}
[[ -s "$ROOTFS/etc/avahi/services/beamer.service" ]] || {
    echo "ERROR: bake.sh did not install the avahi service record. Stations" >&2
    echo "would still resolve by name but nothing could browse for them, so" >&2
    echo "finding one would mean sweeping the subnet by hand." >&2
    exit 1
}
[[ -L "$ROOTFS/etc/systemd/system/multi-user.target.wants/avahi-daemon.service" ]] || {
    echo "ERROR: avahi-daemon.service is not enabled in the image (see bake.sh)." >&2
    echo "The service record above would never be published." >&2
    exit 1
}
[[ ! -e "$ROOTFS/usr/bin/cloud-init" ]] || {
    echo "ERROR: cloud-init is still installed; bake.sh should have purged it." >&2
    exit 1
}

# --- 9. shrink and compress -----------------------------------------------
say "unmounting"
umount -R "$ROOTFS"

say "shrinking"
e2fsck -fy "${LOOP}p2" || true
MIN_BLOCKS=$(resize2fs -P "${LOOP}p2" 2>/dev/null | grep -oE '[0-9]+$')
resize2fs "${LOOP}p2" "$((MIN_BLOCKS + 16384))"
e2fsck -fy "${LOOP}p2" || true

BLOCK_SIZE=$(dumpe2fs -h "${LOOP}p2" 2>/dev/null | sed -n 's/^Block size: *//p')
FS_BLOCKS=$(dumpe2fs -h "${LOOP}p2" 2>/dev/null | sed -n 's/^Block count: *//p')
P2_START=$(sfdisk -J "$IMG" | python3 -c \
    'import json,sys; print(json.load(sys.stdin)["partitiontable"]["partitions"][1]["start"])')
[[ -n "$BLOCK_SIZE" && -n "$FS_BLOCKS" && -n "$P2_START" ]] || {
    echo "ERROR: could not read geometry back (bs=$BLOCK_SIZE blocks=$FS_BLOCKS start=$P2_START)" >&2
    exit 1
}
P2_SECTORS=$(( FS_BLOCKS * BLOCK_SIZE / 512 ))
losetup -d "$LOOP"; LOOP=
sfdisk --no-reread -N 2 "$IMG" <<<"start=$P2_START, size=$P2_SECTORS" >/dev/null
truncate -s "$(( (P2_START + P2_SECTORS) * 512 ))" "$IMG"

OUT="$DIST/beamer-$(date +%Y-%m-%d)-$TARGET.img.xz"

# --- 10. digest sidecar ---------------------------------------------------
say "hashing uncompressed image"
EXTRACT_SIZE=$(stat -c %s "$IMG")
EXTRACT_SHA256=$(sha256sum "$IMG" | cut -d' ' -f1)
printf '{"extract_size": %s, "extract_sha256": "%s"}\n' \
    "$EXTRACT_SIZE" "$EXTRACT_SHA256" > "$OUT.meta"

say "compressing to $(basename "$OUT") at -$XZ_LEVEL"
xz -T0 -"$XZ_LEVEL" -c "$IMG" > "$OUT"

say "done: $OUT"
ls -lh "$OUT"
