#!/bin/bash
# Normally run inside the chroot by build-image.sh. Also safe to run directly as
# root on a fresh (stock) Raspberry Pi OS install.  Follow it with station-init.sh.
#
# This is a provisioning tool - NOT a migration tool. To move a card to a new version
# reflash it.
#
# Deliberately NOT here (see station-init.sh): anything that depends on which
# board it is running on, and anything that needs a real UDC.
set -euo pipefail

SCRIPTS=${SCRIPTS:-$(cd "$(dirname "$0")/../scripts" && pwd)}
CONF=/etc/mtools.conf
IMAGE=/srv/gadget.img

BEAMER_USER=${BEAMER_USER:-beamer}
BEAMER_USER_PASS=${BEAMER_USER_PASS:-}
BEAMER_ARCH=${BEAMER_ARCH:-unknown}

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }
[[ -d "$SCRIPTS/systemd" ]] || { echo "ERROR: no scripts dir at $SCRIPTS" >&2; exit 1; }

[[ -n "$BEAMER_USER_PASS" ]] || {
    echo "ERROR: set BEAMER_USER_PASS." >&2
    echo "This image bakes its own login - Imager's user/SSH fields no longer" >&2
    echo "apply - so without it the station is unreachable." >&2
    exit 1
}

# systemd-detect-virt is the reliable chroot test; ischroot(1) is not present
# everywhere and returns 2 when it cannot tell.
in_chroot() { [[ "$(systemd-detect-virt --chroot 2>/dev/null || echo none)" != "none" ]]; }

# --- packages -------------------------------------------------------------
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
    mtools rsync lighttpd iw python3 iputils-ping rfkill avahi-daemon

# --- mtools config --------------------------------------------------------
cat > "$CONF" <<EOF
mtools_skip_check=1

drive g:
  file="${IMAGE}"
  partition=1
EOF

# --- cloud-init -----------------------------------------------------------
apt-get purge -y cloud-init
rm -rf /etc/cloud /var/lib/cloud

# --- login ----------------------------------------------------------------
if ! id -u "$BEAMER_USER" >/dev/null 2>&1; then
    useradd -m -s /bin/bash "$BEAMER_USER"
fi

for g in sudo adm dialout cdrom audio video plugdev games users input netdev gpio i2c spi; do
    getent group "$g" >/dev/null 2>&1 && adduser "$BEAMER_USER" "$g" >/dev/null 2>&1 || true
done

printf '%s:%s\n' "$BEAMER_USER" "$BEAMER_USER_PASS" | chpasswd

cat > /etc/sudoers.d/010-beamer <<EOF
$BEAMER_USER ALL=(ALL) NOPASSWD: ALL
EOF
chmod 0440 /etc/sudoers.d/010-beamer

# --- NetworkManager -------------------------------------------------------
mkdir -p /etc/NetworkManager/conf.d
cat > /etc/NetworkManager/conf.d/20-beamer-no-connectivity-check.conf <<'EOF'
# See bake.sh. A beamer only needs to reach the AP, never the internet.
[connectivity]
enabled=false
EOF

# --- lighttpd -------------------------------------------------------------
cat > /etc/lighttpd/conf-enabled/98-beamer-api.conf <<'EOF'
server.modules += ( "mod_alias", "mod_cgi" )

# The status check rewrites index.json and adds replay files under lighttpd's
# feet. This station serves a handful of files to a handful of clients, so a
# stat per request costs nothing next to being wrong.
server.stat-cache-engine = "disable"

$HTTP["url"] =~ "^/(status|reset-beamer)$" {
    alias.url += ( "/status"       => "/usr/local/lib/beamer/cgi/beamer-api.cgi",
                   "/reset-beamer" => "/usr/local/lib/beamer/cgi/beamer-api.cgi" )
    cgi.assign = ( ".cgi" => "" )
}

$HTTP["url"] =~ "^/SLIPPI/" {
    index-file.names += ( "index.json" )
}
EOF
rm -f /etc/lighttpd/conf-enabled/99-beamer-dirlisting.conf

install -d -m 0755 /etc/polkit-1/rules.d
install -m 0644 "$SCRIPTS/beamer-web.rules" /etc/polkit-1/rules.d/50-beamer-web.rules
[[ -s /etc/polkit-1/rules.d/50-beamer-web.rules ]] || {
    echo "ERROR: the polkit rule did not install; /status and /reset-beamer" >&2
    echo "would be unable to do anything privileged." >&2
    exit 1
}

mkdir -p /var/www/html
rm -f /var/www/html/index.lighttpd.html
install -d -m 0755 /var/www/html/SLIPPI
cat > /var/www/html/SLIPPI/index.json <<'EOF'
{"schema": 1, "station": "unknown", "generated": null, "count": 0, "files": []}
EOF
chmod 0644 /var/www/html/SLIPPI/index.json

lighttpd -t -f /etc/lighttpd/lighttpd.conf >/dev/null || {
    echo "ERROR: lighttpd rejects its configuration - see 98-beamer-api.conf" >&2
    exit 1
}

# --- avahi ----------------------------------------------------------------
install -d -m 0755 /etc/avahi/services
cat > /etc/avahi/services/beamer.service <<'EOF'
<?xml version="1.0" standalone='no'?><!--*-nxml-*-->
<!DOCTYPE service-group SYSTEM "avahi-service.dtd">
<!-- See bake.sh. %h is this station's hostname, which beamer_apply_hostname
     derives from STATION-NAME and falls back to the station ID for. So this
     file is identical on every card and each station still announces itself
     under its own name. -->
<service-group>
  <name replace-wildcards="yes">%h</name>
  <service>
    <type>_beamer._tcp</type>
    <port>80</port>
  </service>
</service-group>
EOF
chmod 0644 /etc/avahi/services/beamer.service

python3 -c 'import sys, xml.dom.minidom; xml.dom.minidom.parse(sys.argv[1])' \
    /etc/avahi/services/beamer.service || {
    echo "ERROR: /etc/avahi/services/beamer.service is not well-formed XML." >&2
    echo "avahi would drop it without a word and no station would be" >&2
    echo "discoverable." >&2
    exit 1
}

mkdir -p /etc/systemd/system/avahi-daemon.service.d
cat > /etc/systemd/system/avahi-daemon.service.d/10-beamer-late.conf <<'EOF'
# See bake.sh. Discovery is a WiFi-side convenience. It must not compete for
# first-touch SD reads with the path to binding the gadget.
[Unit]
After=gadget.service
EOF

# --- volatile journal -----------------------------------------------------
mkdir -p /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/10-beamer.conf <<'EOF'
# See bake.sh. The journal lives in /run and reaches the SD card only when an
# error persists it - see beamer_persist_journal in beamer-common.sh.
[Journal]
Storage=volatile
RuntimeMaxUse=32M
EOF
if ! in_chroot; then
    systemctl restart systemd-journald
fi
rm -rf /var/log/journal

# --- services a station never uses ----------------------------------------
for u in bluetooth.service hciuart.service ModemManager.service \
         triggerhappy.service triggerhappy.socket dphys-swapfile.service \
         apt-daily.service apt-daily.timer \
         apt-daily-upgrade.service apt-daily-upgrade.timer \
         man-db.timer e2scrub_all.timer e2scrub_reap.service \
         keyboard-setup.service console-setup.service \
         udisks2.service rpi-resize-swap-file.service \
         rpi-eeprom-update.service \
         userconfig.service; do
    systemctl mask "$u" >/dev/null 2>&1 || true
    if [[ "$(readlink -f "/etc/systemd/system/$u" 2>/dev/null)" != "/dev/null" ]]; then
        ln -sfn /dev/null "/etc/systemd/system/$u"
    fi
    [[ "$(readlink -f "/etc/systemd/system/$u")" == "/dev/null" ]] || {
        echo "ERROR: could not mask $u" >&2
        exit 1
    }
done

mkdir -p /etc/systemd/system/regenerate_ssh_host_keys.service.d
cat > /etc/systemd/system/regenerate_ssh_host_keys.service.d/10-beamer-late.conf <<'EOF'
# See bake.sh.
[Unit]
After=gadget.service
EOF

mkdir -p /etc/systemd/system/lighttpd.service.d
cat > /etc/systemd/system/lighttpd.service.d/10-beamer-late.conf <<'EOF'
# See bake.sh. The web root is only reachable over WiFi; starting earlier buys
# nothing and costs the gadget path.
[Unit]
After=network.target
EOF

# --- slp-peek -------------------------------------------------------------
install -d -m 0755 /usr/local/lib/beamer
apt-get install -y --no-install-recommends gcc libc6-dev
cc -Os -Wall -Wextra -Werror -o /usr/local/lib/beamer/slp-peek "$SCRIPTS/slp-peek.c"
strip /usr/local/lib/beamer/slp-peek
chmod 0755 /usr/local/lib/beamer/slp-peek
SLP_RC=0
/usr/local/lib/beamer/slp-peek >/dev/null 2>&1 || SLP_RC=$?
[[ $SLP_RC -eq 2 ]] || {
    echo "ERROR: slp-peek built but does not run (exit $SLP_RC)" >&2
    exit 1
}
apt-get purge -y gcc libc6-dev
apt-get autoremove -y --purge

# --- the build stamp ------------------------------------------------------
install -d -m 0755 /etc/beamer
printf '%s\n' "$BEAMER_ARCH" > /etc/beamer/arch
chmod 0644 /etc/beamer/arch

# --- scripts and units ----------------------------------------------------
install -d -m 0755                                               /usr/local/lib/beamer
install -m 0644 "$SCRIPTS/beamer-common.sh"                      /usr/local/lib/beamer/
install -d -m 0755                                               /usr/local/lib/beamer/cgi
install -m 0755 "$SCRIPTS/beamer-api.cgi"                        /usr/local/lib/beamer/cgi/
install -m 0755 "$SCRIPTS/gadget-up.sh"                          /usr/local/sbin/
install -m 0755 "$SCRIPTS/gadget-down.sh"                        /usr/local/sbin/
install -m 0755 "$SCRIPTS/reset-gadget-data.sh"                  /usr/local/sbin/
install -m 0755 "$SCRIPTS/flush-gadget-data.sh"                  /usr/local/sbin/
install -m 0755 "$SCRIPTS/gadget-eject-watch.sh"                 /usr/local/sbin/
install -m 0755 "$SCRIPTS/make-fs.sh"                            /usr/local/sbin/
install -m 0755 "$SCRIPTS/station-init.sh"                       /usr/local/sbin/
install -m 0755 "$SCRIPTS/status-check.sh"                       /usr/local/sbin/
install -m 0755 "$SCRIPTS/health-check.sh"                       /usr/local/sbin/
install -m 0755 "$SCRIPTS/load-conf.sh"                          /usr/local/sbin/
install -m 0755 "$SCRIPTS/beamer-wifi-apply.sh"                  /usr/local/sbin/
install -m 0755 "$SCRIPTS/growfs.sh"                             /usr/local/sbin/
install -m 0755 "$SCRIPTS/check-net.sh"                          /usr/local/sbin/
install -m 0755 "$SCRIPTS/beamer-led.sh"                         /usr/local/sbin/
install -d -m 0755                                               /usr/lib/systemd/system-shutdown
install -m 0755 "$SCRIPTS/beamer-led.shutdown"                   /usr/lib/systemd/system-shutdown/
install -m 0644 "$SCRIPTS/systemd/gadget.service"                /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/flush-gadget-data.service"     /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/gadget-eject-watch.service"    /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/wifi-powersave-off.service"    /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-firstboot.service"      /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/status-check.service"          /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/status-check.timer"            /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/health-check.service"          /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/health-check.timer"            /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-reset.service"          /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-preflight.service"      /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-wifi-apply.service"     /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-growfs.service"          /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/check-net.service"             /etc/systemd/system/
install -m 0644 "$SCRIPTS/systemd/beamer-led-error.service"      /etc/systemd/system/

systemctl daemon-reload 2>/dev/null || true
systemctl enable gadget.service
systemctl enable status-check.timer
systemctl enable gadget-eject-watch.service
systemctl enable wifi-powersave-off.service
systemctl enable beamer-firstboot.service
systemctl enable health-check.service
systemctl enable health-check.timer
systemctl enable beamer-preflight.service
systemctl enable beamer-wifi-apply.service
systemctl enable beamer-growfs.service
systemctl enable check-net.service
systemctl enable ssh.service
systemctl enable lighttpd.service
systemctl enable avahi-daemon.service

echo "bake complete"
