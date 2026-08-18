#!/bin/bash
# Build and bind the USB mass storage gadget.
# Run by gadget.service at boot; safe to run by hand.
#
# Does NOT source beamer-common.sh. These files are also 
# writen in beamer-common.sh:
#   /var/lib/beamer/station-id
#   /run/gadget-bind-uptime
set -euo pipefail

STATION=$(cat /var/lib/beamer/station-id 2>/dev/null || true)
[[ -n "$STATION" ]] || { echo "/var/lib/beamer/station-id missing or empty - has station-init.sh run?" >&2; exit 1; }

G=/sys/kernel/config/usb_gadget/beamer
IMAGE=/srv/gadget.img

[[ -d "$G" ]] && /usr/local/sbin/gadget-down.sh

[[ -d /sys/module/libcomposite ]] || modprobe libcomposite

if [[ ! -d /sys/kernel/config/usb_gadget ]]; then
    modprobe -q configfs || true
    mount -t configfs none /sys/kernel/config
fi

mkdir -p "$G"
cd "$G"

# Spoofing a real SanDisk Cruzer Blade.
echo 0x0781 > idVendor
echo 0x5567 > idProduct
echo 0x0200 > bcdUSB

mkdir -p strings/0x409
echo "SanDisk"           > strings/0x409/manufacturer
echo "Cruzer Blade"      > strings/0x409/product
echo "BEAMER-${STATION}" > strings/0x409/serialnumber

mkdir -p configs/c.1/strings/0x409
echo "Config 1" > configs/c.1/strings/0x409/configuration
echo 500        > configs/c.1/MaxPower

mkdir -p functions/mass_storage.0
echo 0        > functions/mass_storage.0/stall
echo 1        > functions/mass_storage.0/lun.0/removable
echo 0        > functions/mass_storage.0/lun.0/ro
echo 0        > functions/mass_storage.0/lun.0/nofua
echo "$IMAGE" > functions/mass_storage.0/lun.0/file

ln -s "$G/functions/mass_storage.0" configs/c.1/

UDC=
for ((i = 0; i < 40; i++)); do      # 40 x 50ms = 2s
    for d in /sys/class/udc/*; do
        [[ -e "$d" ]] && UDC=${d##*/}
        break
    done
    [[ -n "$UDC" ]] && break
    sleep 0.05
done
[[ -n "$UDC" ]] || {
    echo "no UDC appeared in /sys/class/udc after 2s - is dwc2 loaded?" >&2
    exit 1
}

printf '%s\n' "$UDC" > UDC
read -r uptime _ < /proc/uptime
printf '%s\n' "$uptime" > /run/gadget-bind-uptime 2>/dev/null || true
echo "gadget bound to $UDC at ${uptime}s"
