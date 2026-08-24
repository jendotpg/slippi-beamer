# Slippi Beamer

## THIS IS A WIP!

I currently have ONE working beamer and have confirmed it works succesfully with [my fork of replay reporter](https://github.com/jendotpg/replay-manager-for-slippi) - but I haven't tried a fleet yet (waiting for RasPi's to come in). Don't bother buying hardware until I edit this (or just reach out and ask :P).

Major TODO still:

1. set up flint 2! write down exact configuration so its repeatable - perhaps its even worth building an openwrt image.... probably not though?

## Configuring a station

`CONFIG/config.txt` on the `BEAMER` drive is the only thing a TO ever edits. The Beamer reads it in full at every boot. Keys are case-insensitive, blank lines and `#` comments are ignored, and values may be quoted.

| Key                  | Default        | What it does                                                                                                                                                                       |
| -------------------- | -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SSID`               | blank          | The network to join.**Blank means this station has no network**, which is a valid way to run one.                                                                                  |
| `PASSWORD`           | blank          | 8–63 characters. Blank means an open network.                                                                                                                                      |
| `COUNTRY`            | `US`           | Two-letter regulatory domain:`US`, `CA`, `JP`, `GB`...                                                                                                                             |
| `HIDDEN`             | `false`        | Whether the network broadcasts its name.                                                                                                                                           |
| `STATION-NAME`       | the station ID | What to call this station. Appears in`CONFIG/status.txt`, as `station_name` in `GET /status`, and as the station's hostname (slugged - see [Station Identity](#station-identity)). |
| `NUM-REPLAYS-SERVED` | `10`           | How many of the newest replays the station hands out over HTTP. 1 to 16.                                                                                                           |

If there are any issues, they'll be recorded in `CONFIG/error.txt`.

**The file is taken whole or not at all.** One bad value rejects the entire config: the station clears its network, drops every setting this file owns, blinks the error pattern, and writes what was wrong to `CONFIG/error.txt`. It does not keep the good half. It does not substitute a default for the bad half. The `NUM-REPLAYS-SERVED` ceiling of 16 is a real limit.

## Status LED

A Beamer's LED is a live readout of station health. It is the fastest way — and usually the only way — to tell whether a Beamer is actually working.

| Pattern                                    | Meaning                                                                                                  |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| Slow even blink, about once a second       | Booting. Wait a bit - up to 90 seconds if the WiFi is failing. A normal boot should be under 30 seconds. |
| Solid on                                   | Healthy! More precisely, lighttpd is answering on port 80 and sshd is accepting connections.             |
| Fast even blink, about five times a second | Not connected. Something went wrong (or this is a freshly imaged Beamer on its first boot)               |
| Off                                        | The drive was ejected and the station has powered itself off. Safe to unplug.                            |

## Setting up a new Beamer

Download `beamer.rpi-imager-manifest` from the [latest release](https://github.com/jendotpg/slippi-beamer/releases/latest). If you built the image yourself, `image/dist/beamer.rpi-imager-manifest` is your manifest file instead.

Try double-clicking the manifest file - it should open the Raspberry Pi Imager. If it doesn't, open the Raspberry Pi Imager yourself, click App Options -> Content Repository -> EDIT -> Use custom file -> select the manifest file -> APPLY & RESTART.

Then per station:

1. Pick your board - `Raspberry Pi Zero W` - then flash the `Beamer station <date> (armhf)` entry onto a microSD card, and insert that card into the Beamer.
2. Plug the Beamer into a laptop. The `BEAMER` drive appears with a `CONFIG/init-finished.txt` confirming provisioning completed.
3. Fill in `CONFIG/config.txt` with SSID, Password, and Station Name.
   1. See [Configuring a station](#configuring-a-station) for more details on this file.
4. Eject the beamer and wait until the light turns off.
5. Plug the beamer into a wii.
6. **Watch the LED.** If it goes solid your beamer is working and ready to go!
   1. If the LED instead starts blinking really fast, you had some sort of error! Unplug the Beamer from the wii and bring it back to your laptop. You can see the error from the last section under `CONFIG/error.txt`. Note that this is from the LAST session! If you update and replug directly into the laptop without trying on a Wii in between, just watch the light - there will still be an `error.txt`.

## How it works

Linux can run a USB port in **device** mode rather than host mode. The `f_mass_storage` gadget function exposes an ordinary file as a USB Bulk-Only-Transport flash drive. The Wii sees a generic USB stick; the "stick" is really a 1 GB disk image sitting on the Pi's SD card.

The critical constraint: the Beamer's OS may **read** the image but must **never write** to it while a host has it mounted. The Wii caches FAT metadata and free-cluster state; a host-side write leaves that cache stale and the Wii's next write lands in the wrong place. All Beamer-side access is read-only, through mtools, which parses FAT in userspace and never invokes the kernel filesystem driver.

**Never `mount` the image, not even `-o ro`. The kernel vfat driver assumes it owns the device and caches metadata aggressively; it will serve stale directory listings.** There are three supported exceptions, and all of them work by writing only when no host can possibly hold the medium - see [Filesystem odds and ends below](#Filesystem-odds-and-ends)

## Hardware

| Item                    | Detail                                                                                                                                                                                                                              | Where I Source Them                                                                                                                                                        |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Raspberry Pi Zero W     | Takes`armhf` image                                                                                                                                                                                                                  | [www.newark.com/raspberry-pi/sc0020/single-board-computer-arm-cortex/dp/69AK9092](https://www.newark.com/raspberry-pi/sc0020/single-board-computer-arm-cortex/dp/69AK9092) |
| microSD card            | 8 GB+ (shoot for 16GB - they're barely more expensive and will last much longer).                                                                                                                                                   | [www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793](https://www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793)                                     |
| USB-A → micro-USB cable | Into the Pi's**`USB`** port (the inner one, nearer the HDMI connector) — **not** `PWR`. Use a real data cable: many heavy "fast charge" cables have no data lines and are invisible to the host. **Power and data share one cable** | [www.cableleader.com/0-5ft-usb2-0-a-male-to-micro-b-male-cable-black.html](https://www.cableleader.com/0-5ft-usb2-0-a-male-to-micro-b-male-cable-black.html)               |
| Case                    | yeah... im still working on this lol....                                                                                                                                                                                            |                                                                                                                                                                            |

This should come out to about \$30 ($16 pi + $7 sd card + $2 cable + $5 case) a unit at time of writing. Depending on venue and size of fleet, you may need to buy a separate router as well - not all WiFi networks can handle an extra 20 devices and very few can handle an extra 80! I use [The GL.Inet Flint 2](https://www.gl-inet.com/en-us/products/gl-mt6000) (~\$170 at time of writing). **Don't count on any of this as battle test yet - reach out to me if you want to start building these and I'll let you know what works and what doesn't.**

Other boards will likely work here - in particular the Zero 2 W, 3 Model A+, 4 Model B, 400, 5, 500 - but the Zero W is the only one any of this is tested on. Everything except the Zero W and Zero 2 W needs its own power supply on top of the cable to the Wii: their USB device port cannot also power the board. The 400 and 500 have no activity LED, so the status readout the rest of this document leans on is simply dead there. They'll also need different data cables depending on which board you try. Reach out to me if you want to do this - I can walk you through the process.

## Repository layout

```
slippi-beamer/
├── README.md									this document
├── image
│   ├── build
│   │   └── ...									dev-box-side build scripts + VM spec
│   ├── dist
│   │   └── ...									images and manifest file for rpi imager
│   ├── scripts
│   │   └── ...									beamer-side scripts and services
│   ├── linux-build.sh							linux build script, used by CI
│   └── mac-build.sh							mac build script
├── .github
│   └── workflows/publish.yml					tag -> image -> draft release
└── tools
    └── fake-beamer.py							pretend to be a station, for replay reporter dev
```

### Testing without a station

Everything `replay-manager-for-slippi` talks to is an mDNS advertisement and four HTTP endpoints — no gadget, no LED, no Wii, so the app's fleet view can be developed and tested on a laptop. Build `slp-peek` natively first; it is portable C and compiles anywhere:

```bash
cc -Os -Wall -o /tmp/slp-peek image/scripts/slp-peek.c
```

```bash
tools/fake-beamer.py --name beamer-stream-1 --port 8081 --replays ~/slp/stream1 --game ~/slp/live.slp
```

Run several on different ports to simulate a fleet — a client should honour the port a station advertises. `--unhealthy` and `--unreported` produce the two known failure states of `/status`.

## HTTP API

Everything a station will tell you, and the one destructive thing it will do for you, over the tournament WiFi. All responses are JSON. There is no authentication: anyone who can reach the station over HTTP can read its status and — with the confirm header below — wipe its replay drive.

| Method | Path             | What it does                                                                |
| ------ | ---------------- | --------------------------------------------------------------------------- |
| `GET`  | `/status`        | The last status report, straight off disk. Runs nothing, so poll it freely. |
| `POST` | `/status`        | Re-runs`status-check.sh`, then returns the fresh report.                    |
| `GET`  | `/SLIPPI/`       | Index of the replays this station is currently serving.                     |
| `GET`  | `/SLIPPI/<file>` | The replay itself.                                                          |
| `POST` | `/reset-beamer`  | Wipes the replay drive. Requires`X-Beamer-Confirm: reset`.                  |

```bash
curl -s http://beamer-3f2a….local/status | python3 -m json.tool
```

```bash
curl -s -d '' http://beamer-3f2a….local/status | python3 -m json.tool
```

```bash
curl -s http://beamer-3f2a….local/SLIPPI/ | python3 -m json.tool
```

```bash
curl -s -d '' -H 'X-Beamer-Confirm: reset' http://beamer-3f2a….local/reset-beamer
```

The `-d ''` is not decoration. lighttpd answers **411 Length Required** to a POST that carries neither a body nor a `Content-Length` (to stop accidents, not attackers), and plain `curl -X POST` sends neither — the request never reaches the station's own code. `-d ''` sends an empty body with a length, and makes the request a POST on its own.

### Discovery

Every station advertises `_beamer._tcp` on port 80 over mDNS, and the instance name is its hostname — so a station shows up as `beamer-stream-station-2` once `STATION-NAME` is set, and as `beamer-<uuid>` before that.

### `GET /status`

Whatever the last check found. `generated` is how you tell how old that is. The gadget and game fields are refreshed every 10 s by `status-check`, so they are never more than a tick behind; the network and daemon fields come from `health-check` on its own 60 s timer, so those can be up to a minute old. `POST` the same URL to refresh the fast half on demand.

The `game` field is `null` when the drive holds no readable replay. Otherwise `live` says whether that replay is still being written, and `ports` carries the character, costume colour and nametag of each occupied port:

```json
  "game": {"live": true, "ports": [
    {"port": 1, "char": "Sheik", "char_id": 19, "color": "white", "costume": 4, "nametag": "\u30b8\u30a7\u30f3"},
    {"port": 2, "char": "Falco", "char_id": 20, "color": "green", "costume": 3, "nametag": null}
  ]},
```

`char` and `color` are the human readable versions of `char_id` and `costume` respectively.

```json
{
  "schema": 1,
  "generated": "2026-08-14T18:22:01Z",
  "station": "3f2a…",
  "station_name": "stream station 2",
  "host": "beamer-3f2a…",
  "boot": "2026-08-14T15:10:41Z",
  "uptime_s": 11520,
  "wifi": "nycmelee",
  "network": "ok",
  "ip": "192.168.1.42",
  "slippi_files": 47,
  "slippi_files_capped": false,
  "udc": "20980000.usb",
  "bind_time_s": 12.4,
  "host_state": "configured",
  "mtools": true,
  "httpd": true,
  "sshd": true,
  "mdns": true,
  "result": "pass",
  "errors": []
}
```

### `GET /SLIPPI/`

Written by the flush, not generated per request, and only rewritten when the set of published replays actually changes.

Only _finished_ games appear here. A replay still being written is held back until it finalizes, because `replay-manager-for-slippi` cannot parse an unfinalized `.slp`; `/status` is where you look to see the game that is being played right now.

```json
{
  "schema": 1,
  "station": "3f2a…",
  "generated": "2026-08-14T18:22:01Z",
  "count": 2,
  "files": [
    {
      "name": "Game_20260814T181203.slp",
      "size": 412393,
      "mtime": "2026-08-14T18:22:01Z",
      "url": "/SLIPPI/Game_20260814T181203.slp"
    }
  ]
}
```

Four things worth knowing before you build anything on this:

- **`count` and `slippi_files` are different numbers on purpose.** `count` is what this station is serving over HTTP, which the flush caps at the newest `NUM-REPLAYS-SERVED` (default 10, see [Configuring a station](#configuring-a-station)). `slippi_files` in `/status` is how many are on the gadget drive, which is usually far more.
- **`slippi_files` saturates at 2000**, reporting exactly `2000` with `"slippi_files_capped": true`. FAT32 stores no file count, so counting means walking the directory. Measured on a Zero W that walk is about 175 ms at 2000 files and 150 ms at 320 — essentially flat, because forking `mdir` costs more than the walk does. The cap is above what a 1 GB drive can physically hold, so the number is exact in practice; it exists to bound the pathological case, not the normal one.
- **`mtime` is when the Beamer published the file, which is within about 10 s of the game ending.** A replay is copied exactly once, on the first tick after it finalizes, and never touched again - so `mtime` is a good proxy for when the game ended, but it is still not the FAT timestamp (`mcopy` does not carry that across). Parse `Game_<timestamp>.slp` from the name if you need the time the game _started_.
- **Only `*.slp` is listed or served.** Any host that is not a Wii leaves droppings on the drive — a Mac writes `.DS_Store` into a folder Finder opens, and a `._` twin beside each file it copies — and none of that is a replay.

### Odds and ends

`GET /` returns `403`. There is no page at the root and no directory listing anywhere; that is the endpoint working, not a broken station.

Both POSTs take a lock, so a reset can never overwrite the image while a status check is reading it. The loser gets `409` rather than a request that hangs.

A reset does not refresh the cached `/status`, so `slippi_files` there stays stale until the next tick or `POST /status`. That is deliberate — it keeps the reset request fast.

`POST /status` runs `status-check` only, never `health-check`. Anything that reached this endpoint has already proved the network works, so re-probing it would put a multi-second budget in the request path to answer a question the request itself just answered.

## Beamer OS Image

### Build

The image bakes its own login, so the build needs to be told what it is:

```Shell
BEAMER_USER_PASS='password' ./image/mac-build.sh
```

or

```Shell
BEAMER_USER_PASS='password' ./image/linux-build.sh
```

`BEAMER_USER` renames the account, which defaults to `beamer`. The build **fails** without a password: a station has no console and is bus-powered by the console it is bolted to, so an image with no way in is an image you have to reflash to debug. The account gets passwordless `sudo`, the same shape stock Raspberry Pi OS uses for the account Imager creates.

### Releasing

Pushing a `v*` tag builds the image in GitHub Actions and leaves a **draft** release carrying the `.img.xz`, its `.meta` sidecar, and a manifest whose URLs point at that release.

```bash
git tag v1.2.3 && git push origin v1.2.3
```

Released images carry the login `beamer` / `password`. If this makes you uncomfortable security-wise, feel free to build your own image. I don't think its an issue - everyone in the venue has physical access to the Pi already :P

### Details

#### Basics

Raspberry Pi OS Lite, pinned by URL and SHA-256 in `image/build/targets/armhf.conf`. Further details are baked into the image at build time.

cloud-init is purged rather than disabled. On a stock image it exists only to apply the Imager customisation screen's settings, and a Beamer needs none of them: it derives its own identity, deletes any WiFi profile it did not write, and now carries its own login. What it cost was a full Python startup on a single 1 GHz ARM11 core, on the critical path of every boot.

#### Filesystem odds and ends

An 8 GB card is the floor. Two of the numbers behind that are hard caps rather than estimates, so they are worth writing down.

| what                              | size                                            | pinned by                                                     |
| --------------------------------- | ----------------------------------------------- | ------------------------------------------------------------- |
| boot partition                    | 512 MiB                                         | stock Raspberry Pi OS layout                                  |
| rootfs as shipped                 | 1978 MiB — its own minimum plus 64 MB of slack  | `resize2fs -P`, then `MIN_BLOCKS + 16384` in `build-image.sh` |
| rootfs ceiling once grown         | **8192 MiB**, no matter how large the card is   | `MAX_ROOT_MB` in `growfs.sh`                                  |
| `/srv/gadget.img`                 | 1 GiB once the medium fills with replays        | `SIZE=1G` in `make-fs.sh`                                     |
| `/var/lib/beamer/journald_dumps/` | 512 MB — 16 dumps against the 32 MB journal cap | `BEAMER_JOURNAL_KEEP`, `RuntimeMaxUse` in `bake.sh`           |
| everything else                   | negligible                                      |                                                               |

The following are the **only** supported times that it is valid to mount the filesystem image:

1. `scripts/reset-gadget-data.sh` ejects the medium first, so the host is forced to re-read rather than trusting its cache.
2. `load-conf.sh` and `station-init.sh` write `CONFIG/` in the **pre-bind window** — they run from `beamer-preflight.service` and `beamer-firstboot.service`, both ordered `Before=gadget.service`, so the image has not yet been handed to a UDC and nothing is attached to it. This window is the reason those two units, and only those two, sit in front of the bind.
3. `scripts/gadget-eject-watch.sh` waits for the **host** to eject. `removable` is set on the LUN, so an eject makes `f_mass_storage` close the backing file; `lun.0/file` reads empty and the image is unattached again. This is the only window in which the Pi can write something it did not know at boot.

| written during the session | rotated at next boot into | published by                                              |
| -------------------------- | ------------------------- | --------------------------------------------------------- |
| `error.late.log`           | `error.prev.log`          | `CONFIG/error.txt`, prefixed `[previous boot]`            |
| `status.late`              | `status.prev`             | `CONFIG/status.txt`, suffixed `(as of the previous boot)` |

Both live in `/var/lib/beamer` - the non-volatile half of the Beamer state. The volatile half lives in `/run/beamer` on tmpfs and is rebuilt each boot.

`/var/lib/beamer/journald_dumps/`, within the non-volatile part of the Beamer state, stores journals from past boots. The journal itself is volatile, so the first `beamer_error` of a boot persists the whole journal into a numbered file that boot owns. Later errors in the same boot rewrite that same file, and the next boot takes a file of its own and cannot touch it. Only the newest 16 errored boot journals are kept.

#### Fleet determinism

**A station's behaviour must be a function of its config file and nothing else.** Two cards holding the same `config.txt` have to end up in the same state, byte for byte, no matter what either one was doing before. That is a central promise of the fleet: a TO can swap a card between setups, or reflash one mid-tournament, without learning anything about that particular card's history.

Every tempting little kindness breaks this:

- **Falling back to a default on a bad value.** Now the station's real behaviour is not what its file says, and the TO has no way to see the difference. Reject the file instead.
- **"Kept the previous setting" on a rejected value.** The previous setting came from a boot that may never have happened on the next card. `load-conf.sh` deletes `station-name`, `num-replays` and `wifi-country` from `/run/beamer` on the reject path for exactly this reason — a rejected card matches a rejected card anywhere else, rather than matching its own past.
- **Persisting anything derived from the config across boots without re-deriving it.** Anything a config key owns is a cache of that key, not a second source of truth for it. This is structural: everything `config.txt` owns lives in `/run/beamer` on tmpfs, so it cannot outlive a boot even by accident. Only `/var/lib/beamer` persists, and nothing a config key owns is allowed there — see the two path blocks at the top of `beamer-common.sh`.

There is exactly one deliberate exception, and it is not about values being wrong — it is about there being no values at all. If the image or the config file cannot be _read_ (`no gadget image`, `unreadable`), the station leaves its network settings alone rather than tearing down a working station over a transient mtools failure. `CONFIG/status.txt` says so in as many words.

#### No initramfs

The image ships `auto_initramfs=0` and no `initramfs*` files. Measured on a Zero W, removing the stock 13.9 MB initramfs took **7.5 seconds** off the time to bind — the single largest item in the boot. Almost none of that is loading or unpacking it (the kernel unpacks it in 28 ms); it is the cost of the initramfs _running_ — busybox, udev, fsck, mount, `switch_root` — on a 1 GHz ARM11. Raspberry Pi kernels build in the MMC and ext4 drivers and resolve`root=PARTUUID=` natively, so nothing needs one.

Two things the initramfs used to do had to move:

- **Growing the rootfs.** `build-image.sh` ships a rootfs shrunk to its minimum plus ~64 MB, and the `resize` cmdline token was what grew it. That is now `beamer-growfs.service`, running `growfs.sh` once on first boot, ordered _after_ the gadget binds.
- **fsck.** `systemd-fsck-root.service` used to be skipped (`ConditionPathExists=!/run/initramfs/fsck-root`) because the initramfs had already done it. It now runs, and it is on the critical path — root has to be read-write before `f_mass_storage` can open the backing file. It cost about 0.6 s in the measurement that removed the initramfs, against 8.1 s saved.

Measured on a Zero W - time to the gadget binding:

| stage                       |            |                                                               |
| --------------------------- | ---------- | ------------------------------------------------------------- |
| kernel init                 | 2.97 s     |                                                               |
| systemd startup to`-.mount` | 5.21 s     | reading its binary, libraries and ~200 unit files             |
| `systemd-fsck-root`         | 4.19 s     | **3.62 s of it is loading e2fsck**; the check itself is 0.5 s |
| `systemd-remount-fs`        | 0.93 s     |                                                               |
| `beamer-preflight`          | 1.92 s     | ~1.1 s of script, the rest bash and systemd overhead          |
| `gadget-up`                 | 0.99 s     |                                                               |
| **total**                   | **17.6 s** | from 26.1 s before this work                                  |

The shape of that table is the finding: almost none of it is computation. It is first-touch reads off the SD card, which does 19 MB/s sequentially but far less on the small scattered reads that loading binaries and unit files actually generates. Optimising shell scripts had a real but bounded payoff (`beamer-preflight` went 2.66 s → 1.10 s by removing about 34 fork+execs); the rest of the boot is waiting on the card.

A warning about reading these numbers. `journalctl -k -o short-monotonic` timestamps **kernel** messages with the moment journald drained `/dev/kmsg` at its own startup, not the moment the kernel logged them — so the whole kernel boot appears as a 50 ms cluster somewhere in the middle of userspace, and "the first kernel message" is not the kernel starting. Use `dmesg`, whose clock genuinely starts at `[0.000000]` at kernel start, and `systemd-analyze`'s kernel-versus-userspace split. Two separate wrong conclusions came out of trusting the journald view.

#### Services

Roughly in boot order. Everything on the pre-bind side carries `OnFailure=beamer-led-error.service`.

| Unit                 | What it does                                                    |
| -------------------- | --------------------------------------------------------------- |
| `beamer-firstboot`   | Derives the station identity and stages the gadget image, once. |
| `beamer-preflight`   | Writes`CONFIG/` into the image before it is handed to a UDC.    |
| `gadget`             | Binds the image to a UDC as USB mass storage.                   |
| `beamer-wifi-apply`  | Applies the regulatory domain and unblocks the radio.           |
| `wifi-powersave-off` | Disables WiFi power save.                                       |
| `check-net`          | Verifies the station joined its configured network.             |
| `avahi-daemon`       | Publishes the`_beamer._tcp` record. Ordered after the bind.     |
| `beamer-growfs`      | Grows the root filesystem, once.                                |
| `status-check.timer` | Fires the status check every 10 s while the gadget is bound.    |
| `status-check`       | Gadget state, game-in-progress detection, drives the flush.     |
| `health-check.timer` | Fires the health check every 60 s.                              |
| `health-check`       | Network and daemon probes. Deprioritised to SCHED_IDLE.         |
| `flush-gadget-data`  | Copies finished replays off the image into the web root.        |
| `gadget-eject-watch` | Waits for the host to eject, refreshes status, powers off.      |
| `beamer-reset`       | Wipes the replay drive back to the template.                    |
| `beamer-led-error`   | Puts the LED into the error blink.                              |

#### Scripts

Installed to `/usr/local/sbin` unless noted.

| Script                  | What it does                                                                                      |
| ----------------------- | ------------------------------------------------------------------------------------------------- |
| `station-init.sh`       | Derives the UUID, sets the hostname, lays down the first image.                                   |
| `load-conf.sh`          | Reads`CONFIG/config.txt` and republishes `CONFIG/` in the pre-bind window.                        |
| `gadget-up.sh`          | Builds the configfs gadget and binds it.                                                          |
| `gadget-down.sh`        | Unbinds and tears the gadget down.                                                                |
| `beamer-wifi-apply.sh`  | Sets the regulatory domain and deletes foreign WiFi profiles.                                     |
| `check-net.sh`          | Waits for association and DHCP, then records the outcome.                                         |
| `growfs.sh`             | Grows the root partition and filesystem, capped, once.                                            |
| `status-check.sh`       | Gadget state, which game is being played, and when to flush. Every 10 s.                          |
| `health-check.sh`       | Network and daemon probes into the report. Every 60 s, SCHED_IDLE.                                |
| `flush-gadget-data.sh`  | `mcopy`s _finished_ replays out and rewrites `SLIPPI/index.json`.                                 |
| `slp-peek`              | Reads a replay's first KB: who is playing, and is it still being written.`/usr/local/lib/beamer`. |
| `gadget-eject-watch.sh` | Polls the LUN for an eject, then writes status and shuts down.                                    |
| `reset-gadget-data.sh`  | Ejects the medium and restores the template over the image.                                       |
| `make-fs.sh`            | Builds the blank 1 GB FAT32 template (build-time; also runnable on a Pi).                         |
| `beamer-led.sh`         | Drives the ACT LED into`boot`/`ok`/`error`/`off`.                                                 |
| `beamer-led.shutdown`   | Turns the LED off at shutdown (`/usr/lib/systemd/system-shutdown/`).                              |
| `beamer-api.cgi`        | Serves the`/status` and `/reset-beamer` endpoints (`/usr/local/lib/beamer/cgi/`).                 |
| `beamer-common.sh`      | Holds the shared paths, logging and status helpers (`/usr/local/lib/beamer/`, sourced only).      |
| `beamer-web.rules`      | Lets`www-data` start those two units (`/etc/polkit-1/rules.d/`).                                  |

#### Station Identity

Each Pi assigns its own, at first boot, in `station-init.sh`: a UUIDv5 over the board's CPU serial from `/proc/cpuinfo` against a fixed project namespace, written once to `/var/lib/beamer/station-id` and never overwritten. The USB serial descriptor becomes `BEAMER-<uuid>`.

The **hostname** comes from `STATION-NAME`, not from the UUID: `load-conf.sh` re-derives it on every boot, so renaming a station in `config.txt` renames it on the network at the next power cycle. The name is slugged — lowercased, every run of anything outside `[a-z0-9]` collapsed to one hyphen, trimmed of leading and trailing hyphens, cut to 56 characters — and `beamer-` is prefixed to it. `Stream Station 2` becomes `beamer-stream-station-2`.

As a result, **names that slug to nothing fall back to the ID.** `STATION-NAME=拉拉` is a perfectly good name for `CONFIG/status.txt` and `GET /status`, and it carries there in full — but there is no hostname in it, so that station stays `beamer-<uuid>`.

Nothing enforces uniqueness. Two stations named `Setup 2` claim the same hostname, the same way two hosts on any network would; the UUID underneath them stays distinct, and that is what `GET /status` and the replay index report. Reflashing a board with an updated image will keep the same UUID - it's derived from hardware details!
