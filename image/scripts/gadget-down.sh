#!/bin/bash
# Unbind and tear down the gadget. Safe to run when nothing is up.
#
# Deliberately NOT set -e. This is gadget.service's ExecStop, and that unit
# carries OnFailure=beamer-led-error.service, so a teardown that exits non-zero
# would fail the unit on the way down and light the error blink on a station that
# is merely shutting down. On the shutdown path the kernel is about to drop the
# whole configfs tree anyway, so a stuck rmdir is not a reason to call it bad.
set -uo pipefail

G=/sys/kernel/config/usb_gadget/beamer
[ -d "$G" ] || exit 0

echo "" > "$G/UDC" || true
rm -f "$G"/configs/c.1/mass_storage.0
rmdir "$G"/configs/c.1/strings/0x409 || true
rmdir "$G"/configs/c.1 || true
rmdir "$G"/functions/mass_storage.0 || true
rmdir "$G"/strings/0x409 || true
rmdir "$G" || true

exit 0
