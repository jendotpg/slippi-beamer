# Slippi Beamer

## THIS IS A WIP!

I currently have ONE working raspi beamer and ONE working ESP32 beamer. I have confirmed both can report sets succesfully with [my fork of replay reporter](https://github.com/jendotpg/replay-manager-for-slippi).

Major TODOs still:

1. get replay reporter up to date!
2. make button turn screen upside down!
3. provide a way to update config files OTA
   1. TO custom idle message (to say bo5, stadium frozen, etc) - set it at config time!
   2. when updating config OTA, restart the beamers after update.
   3. is it possible to restart beamer dynamically after config file is written on laptop too?
      1. wait a few seconds,,, need to be able to correct typos ofc

4. clean clippy LOL
5. colorblind mode? amber > blue, perhaps?
6. support other boards with different pinouts! different build options, maybe?
   1. order and test Waveshare ESP32-S3-LCD-1.47 version

## Configuring a station

`CONFIG/config.txt` on the `BEAMER` drive is the only thing a TO ever edits. The Beamer reads it in full at every boot. Keys are case-insensitive, blank lines and `#` comments are ignored, and values may be quoted.

| Key                  | Default        | What it does                                                                                                                                               |
| -------------------- | -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SSID`               | blank          | The network to join.**Blank means this station has no network.**                                                                                           |
| `PASSWORD`           | blank          | 8–63 characters. Blank means an open network.                                                                                                              |
| `COUNTRY`            | `US`           | Two-letter regulatory domain:`US`, `CA`, `JP`, `GB`...                                                                                                     |
| `HIDDEN`             | `false`        | Whether the network broadcasts its name.                                                                                                                   |
| `STATION-NAME`       | the station ID | What to call this station. Appears as`station_name` in `GET /status`, and as the station's hostname (slugged - see [Station Identity](#station-identity)). |
| `NUM-REPLAYS-SERVED` | `10`           | How many of the newest replays the station hands out over HTTP. 1 to 16.                                                                                   |
| `REPLAY-CAP`         | `512`          | How many replays the station counts on the card before it stops counting. 1 to 2048. Past 75% it warns; at the cap it warns and stops serving new replays. |
| `LED-BRIGHTNESS`     | `20`           | The status LED, 0 to 100 percent.                                                                                                                          |
| `DEBUG`              | `false`        | Whether to keep a`LOGS/debug.txt` of each boot. Off means the journal records nothing at all. These files are never deleted automatically.                 |

## Status Readout

A Beamer's screen and LED are live readouts of station health. They are the fastest way — and usually the only way — to tell whether a Beamer is actually working.

**Amber means something is happening — DO NOT UNPLUG.**

| Screen                                                               | LED Pattern                                          | Meaning                                                                           | Safe to Unplug? |
| -------------------------------------------------------------------- | ---------------------------------------------------- | --------------------------------------------------------------------------------- | --------------- |
| An indeterminate loading circle, one rotation per second             | Slow even blink, about once a second (**amber**)     | Booting.                                                                          | no              |
| `WRITING` or `SENDING`, with moving dots, over `DO NOT UNPLUG`       | Solid (**amber**)                                    | Busy. A game is being recorded, or a file is being served.                        | no              |
| The station name, large. IP and how full the card is beneath it.     | Solid (**green**)                                    | Healthy and idle. Everything is on the card.                                      | yes             |
| A warning label, large. One line of advice, then the station name.   | Slow even blink, about once a second (**green**)     | Working but with a warning.                                                       | yes             |
| The error label, large. One line of detail, then where to read more. | Fast even blink, about five times a second (**red**) | Something went wrong (or this is a freshly flashed Beamer on its first boot).     | yes             |
| Dark, backlight off                                                  | Dark                                                 | Ejected and shut down.                                                            | yes             |
| Dark, backlight off                                                  | Solid (**red**)                                      | Stopped. It hit a fault it could not recover from. Unplug it and plug it back in. | yes             |

**Ejecting a Beamer shuts it down for good.** It flushes the card, leaves the network, drops off the USB bus and sleeps. Unplugging and replugging it is the only way to bring it back.

**An errored Beamer (Red LED, ERROR screen) generally acts like a regular USB drive.** Pull it using the same judgement you would as any other USB drive. It will still record games! (This is of course not true with`NO SD CARD`, `SD UNREADABLE`, `WRONG FORMAT`as there's no way to write the games)

The screen shows only the first error of the boot, with`+N more` beneath it when there are others.

## Setting up a new Beamer

1. Insert a microSD card into the dongle formatted as FAT32 with a first partition of 4GB or smaller.
   1. The microSD slot is INSIDE the usb jack! Remove the dummy card that comes inside to insert the new one.
2. Hold the button on the side of the board dongle while you plug it into your laptop, then let go. Then press flash on [the flashing page](https://jendotpg.github.io/slippi-beamer/)
   1. "Leaving..." means its done - you don't have to wait any longer!
3. Unplug and replug the dongle to leave download mode. The first boot derives the station identity and lays down `CONFIG/` and `LOGS/`. A microSD card that is exFAT, unpartitioned, or partitioned larger than 4GB shows `WRONG FORMAT` on the screen — see [Card size](#card-size).
4. Fill in `CONFIG/config.txt` with SSID, Password, and Station Name.
   1. See [Configuring a station](#configuring-a-station) for more details on this file.
5. Eject the Beamer and wait until the light and screen go dark.
6. Plug the Beamer into a Wii and watch the screen/LED. If it goes green and shows the station name your Beamer is working and ready to go!
   1. If it instead shows an error label in large text — and the LED starts blinking fast — that label is your diagnosis. [Error labels](#error-labels) says what each one means. The screen gives you one line of detail; for the full text, unplug the Beamer from the Wii, bring it back to your laptop, and read `LOGS/error.txt`.
   2. Note that `error.txt` is from the LAST session! If you update and replug directly into the laptop without trying on a Wii in between, watch the screen instead — there will still be an `error.txt` and it will be describing the previous boot, not the current one!

## Hardware

| Item                        | Detail                                                                                                                                                                                                                                             | Where I Source Them                                                                                                                    |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| LilyGO T-Dongle-S3 with LCD | ESP32-S3 with 16 MB flash. Native 4-bit SDMMC, native USB OTG on a USB-A male plug, an addressable RGB status LED, a 0.96" 160×80 colour screen, and a transparent case. Get the variant with the screen if you can afford the extra dollar or so! | [www.amazon.com/dp/B0BK9162QY](https://www.amazon.com/dp/B0BK9162QY?lv=shuf&channelId=500&plpRedirect=mhFallback&th=1)                 |
| microSD card                | Any size from 4 GB up, but format the replay partition to 4 GB with 4 KB clusters — see[Card size](#card-size).                                                                                                                                    | [www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793](https://www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793) |

Depending on venue and size of fleet, you may need to buy a separate router as well - not all WiFi networks can handle an extra 20 devices and very few can handle an extra 80! I use [The GL.Inet Flint 2](https://www.gl-inet.com/en-us/products/gl-mt6000) (~$170 at time of writing). One thing to note: **ESP32-S3 is 2.4 GHz only.** Maybe sometime soon we'll see a company selling the ESP32-S31 in the dongle form factor and move over - 5 GHz and WiFi 6 would lowkey be a godsend...

## Testing without a station

Everything `replay-manager-for-slippi` talks to is an mDNS advertisement and five HTTP endpoints — no USB, no LED, no Wii, so the app's fleet view can be developed and tested on a laptop. Nothing needs building first:

```bash
tools/fake-beamer.py --name beamer-stream-1 --port 8081 --replays ~/slp/stream1 --game ~/slp/live.slp
```

The `game` object is not canned: `--game` is peeked out of a real `.slp` by a port of `beamer::slp` carried inside the script, so the character icons in the app are a real test. Run several on different ports to simulate a fleet — a client should honour the port a station advertises. `--unhealthy` and `--unreported` produce the two known failure states of `/status`.

## HTTP API

Everything a station will tell you, and the one destructive thing it will do for you, over the tournament WiFi. All responses are JSON. There is no authentication: anyone who can reach the station over HTTP can read its status and — with the confirm header below — wipe its replay drive.

| Method | Path             | What it does                                                                             |
| ------ | ---------------- | ---------------------------------------------------------------------------------------- |
| `GET`  | `/status`        | The last status report, straight off the two fragments. Runs nothing, so poll it freely. |
| `POST` | `/status`        | Re-runs the scan tick, then returns the fresh report.                                    |
| `GET`  | `/SLIPPI/`       | Index of the replays this station is currently serving. See[Publishing](#publishing).    |
| `GET`  | `/SLIPPI/<file>` | The replay itself.                                                                       |
| `POST` | `/reset-beamer`  | Wipes the replay drive. Requires`X-Beamer-Confirm: reset`.                               |

### Discovery

Every station advertises `_beamer._tcp` on port 80 over mDNS, and the instance name is its hostname — so a station shows up as `beamer-stream-station-2` once `STATION-NAME` is set, and as `beamer-<uuid>` before that.

### `GET /status`

Everything here is cached by the scan tick so this `GET` has minimal cost.`POST` the same URL to rescan on demand. `ssh` is always `false` on esp32; it is in the contract because a Pi genuinely can offer it. `game` is `null` until this station has watched a game start. `live` says whether the last replay is still being written, and `ports` carries the character, costume colour and nametag of each occupied port during the last recorded game. `replay_count` and `replay_cap` are meant to be read together: `replay_count / replay_cap` is how full the drive is, and `replay_count == replay_cap` means counting stopped there. `secs_since_port_change` and `secs_since_character_change` give estimates for how long the set has been running: "how long have players been on these ports" and "how long have players been on these characters" (note that they're only updated when a game starts, so if players plug into the same ports you need to look at character change - but if one of the players is a known counterpicker you should look at ports! if new players plug into the same ports and play the same characters youre screwed.) `health` is `"ok"`, `"starting"`, `"warn"` or `"error"`. `"starting"` means the network has not finished coming up yet. `"warn"` means the `warnings` array is non-empty: the station is still theoretically recording, still serving and still safe to unplug, but something about it is off (usually the replay count is approaching cap or the Wii is failing to mount the Beamer).

```json
{
  "schema": 1,
  "arch": "esp32",
  "station_id": "3f2a...",
  "station_name": "stream station 2",
  "ssid": "nycmelee",
  "replay_count": 47,
  "replay_cap": 512,
  "ssh": false,
  "game": null,
  "secs_since_port_change": null,
  "secs_since_character_change": null,
  "health": "ok",
  "warnings": []
}
```

### `GET /SLIPPI/`

Rendered when the set of published replays changes, never per request. There is no web root and nothing is copied — the index is a list of URLs and the file is served straight off the card. Doesn't include a live game that hasn't been finished

```json
{
  "schema": 1,
  "station_id": "3f2a...",
  "served_replay_count": 2,
  "files": [
    {
      "size": 412393,
      "url": "/SLIPPI/Game_20260814T181203.slp"
    }
  ]
}
```

A few notes:

- `served_replay_count` is how many replays are **retrievable**; `replay_count` is how many are on the drive
- `replay_count` stops at `replay_cap` (`REPLAY-CAP`, default 512) for performance reasons - directory walks are expensive!
- Only `*.slp` are listed or served and filenames can't have spaces or unexpected special characters - this is meant for reading off of a Wii!

### Odds and ends

`GET /` returns `403`. There is no page at the root and no directory listing anywhere; that is the endpoint working, not a broken station.

Both POSTs take a lock, so a reset and a status refresh can never be in flight at once. The loser gets `409` immediately rather than a request that hangs.

A reset is refused with `409` while a replay is going out, because unlinking a file an in-flight download has open truncates it without either end being told — and somebody mid-collection is exactly who a reset would hurt. Retry once the transfer finishes.

A reset takes the medium away and gives it straight back - this will confuse a lot of hosts! If you're mounting to a computer instead of a Wii, just delete the replays yourself.

## Beamer firmware

### Build

There is one build and it targets the chip. `.cargo/config.toml` at the repo root selects `xtensa-esp32s3-espidf`, the `ldproxy` linker and the pinned ESP-IDF version, and cargo finds it by walking up from wherever you are — so `cargo build` means the firmware from any directory in the tree, and there is nothing else it could mean.

It needs the esp-rs toolchain and ESP-IDF. Once, on a new machine:

```bash
cargo install espup espflash ldproxy && espup install
```

`espup` installs the xtensa Rust fork as a rustup toolchain and writes `~/export-esp.sh`, which has to be sourced in each new shell. `ldproxy` is a separate install and is easy to miss — without it the build gets all the way to the link step and then says `linker 'ldproxy' not found`. Then:

```bash
source ~/export-esp.sh && cargo build --release
```

Editing `components/beamer_msc/` does not, on its own, rebuild it. esp-idf-sys drives the entire ESP-IDF CMake build from its own build script, and that script tracks exactly two things: the `sdkconfig` and `sdkconfig.defaults` files. It does not track the directories named in `extra_components`. So a change to the C shim gives cargo no reason to re-run it, CMake is never invoked, the previous object file is linked, and the build succeeds with a clean log and flashes a binary that is not the code in the tree.

`build.rs` refuses to let that happen: it hashes `components/`, and on a change it bumps the mtime of `sdkconfig.defaults` and fails the build, so the next one is correct. If you ever need to force it by hand:

```bash
touch sdkconfig.defaults && cargo build --release
```

One command builds, flashes and opens the serial monitor. `espflash flash` takes the ELF, and `.cargo/config.toml` sets it as the cargo runner, so `cargo run` does all three. The partition table and the board's 16 MB flash size come from `espflash.toml`, so there are no flags to remember:

```bash
cargo run --release
```

Hold the button on the side of the board while plugging it in to enter download mode. That is not optional after the first flash: the firmware binds USB Mass Storage about 200 ms into the boot, and from that moment the serial port is gone, so espflash has nothing to reset into download mode. Every reflash needs the button.

### Error labels

| Label           | What happened                                                                                             |
| --------------- | --------------------------------------------------------------------------------------------------------- |
| `NO ID`         | The board's factory MAC is unset or all zeroes. Refusing to boot.                                         |
| `NO SD CARD`    | The card slot came up empty, or no card responded.                                                        |
| `SD UNREADABLE` | A card is present but its filesystem will not mount.                                                      |
| `WRONG FORMAT`  | A card is readable, but has no FAT32 partition or a first partition over 4 GB.                            |
| `NO CONFIG`     | `CONFIG/config.txt` could not be read.                                                                    |
| `BAD CONFIG`    | The file was read and rejected. The detail line names the bad key.                                        |
| `NO USB`        | The USB stack would not start. The station halts. (A host that simply never reads is`NO WII`, a warning.) |
| `NO WIFI`       | Did not associate with the configured SSID.                                                               |
| `WRONG WIFI`    | Associated, but with a different network than`config.txt` asks for.                                       |
| `NO IP`         | Associated, but the network handed out no address.                                                        |
| `NO HTTP`       | Nothing answered on port 80. It is collecting replays it cannot serve.                                    |
| `NO MDNS`       | It will not appear in a discovery browse. Replays are unaffected.                                         |
| `CRASHED`       | The firmware panicked. The faulting task is parked; the station is still recording.                       |

### Warning labels

| Label           | What is off                                                                                              |
| --------------- | -------------------------------------------------------------------------------------------------------- |
| `DRIVE FAILING` | The card has stopped answering reads. Replays are still recorded, but not counted or served.             |
| `DRIVE FULL`    | `REPLAY-CAP` replays are on the card. New ones are no longer served. Delete some.                        |
| `NO WII`        | Nothing has read this drive in ten seconds — a charger, a dead port, or a console that never mounted it. |
| `SLP MISFORMAT` | A replay on the card will not parse. It is counted but never served; the station is otherwise fine.      |
| `DRIVE FILLING` | The card is past 75% of`REPLAY-CAP`. Delete replays before it stops serving new ones.                    |

### A warning about FAT cache

The Wii's FAT cache lives in the Wii's memory: write a directory entry while the Wii holds the medium and the Wii's next writeback clobbers it, or both of you allocate the same clusters and a replay is lost. So the firmware writes to the volume in exactly two windows, and both work by writing only when no host can possibly hold the medium:

1. Before the USB bind at boot, which is why the config is read early rather than when it is first needed. See [Boot time](#boot-time).
2. After the host ejects, which is a clean SCSI media-change the firmware is told about. See [Eject and the durability promise](#eject-and-the-durability-promise).

Everything else — serving replays over HTTP, counting files, peeking at the game in progress — is strictly read-only, and re-reads the FAT rather than trusting anything it read on a previous tick.

### Boot phases

| Phase                      | What                                                                                                                                                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ClaimIdentity`            | eFuse base MAC becomes a UUIDv5. Refuse to boot on an all-zero MAC. Spawn the status task, so the LED lights at once.                                                                                               |
| `MountCard`                | `sdmmc_host_init`, 4-bit bus, probe the card.                                                                                                                                                                       |
| `PrepareForBind`           | **The write window.** Mount FatFs read-write, read `CONFIG/config.txt`, rotate the error state, mirror `LOGS/error.txt`, write `LOGS/debug.txt` if `DEBUG` is set, seed a template `config.txt` if absent, unmount. |
| `BindCard`                 | Present the card to the host as a USB drive.                                                                                                                                                                        |
| `StartJournal`             | The journal drain, the read window, the scan tick.                                                                                                                                                                  |
| `EstablishNetworkServices` | WiFi,`esp_http_server`, mDNS `_beamer._tcp` on 80.                                                                                                                                                                  |
| `Running`                  | The verdict loop. The boot is over.                                                                                                                                                                                 |

### Memory

The main memory constraint is the largest free block, not the number of free bytes. Both numbers are logged, at boot, when the network comes up, in every periodic journal summary, and beside any mount failure. They arrive as `heap: N B free, largest block M B` in `LOGS/debug.txt`, when `DEBUG` is set. A healthy station settles around 60 KB free, and the gap between the two figures is the fragmentation. The journal summary also carries the smallest that the largest free block ever got:`low water K B.`

#### Allocated statically at link time

| Consumer                   |      Bytes |                                                                                                   |
| -------------------------- | ---------: | ------------------------------------------------------------------------------------------------- |
| `beamer_wbc.c` `s_data`    |     32,768 | the write-back cache: 64 sectors of 512 B                                                         |
| `beamer_wbc.c` `s_staging` |      8,192 | one flush run, DMA'd straight out of`.bss`                                                        |
| `beamer_msc.c` `s_ring`    |      8,192 | 512 transfer timings, the CBW→CSW census                                                          |
| `beamer_log.c` `s_ring`    |      8,192 | the`esp_log` capture that becomes `LOGS/debug.txt`. Static - `DEBUG=false` does not give it back. |
| `lcd.rs` `SCRATCH`         |      7,680 | one 160×24 band of the panel, so rendering never allocates                                        |
| `http.rs` `SEND_BUF`       |      8,192 | the replay read chunk — see below                                                                 |
| TinyUSB`_mscd_epbuf`       |      4,096 | `CFG_TUD_MSC_EP_BUFSIZE`                                                                          |
| `beamer_wbc.c` `s_meta`    |        768 | 64 slot descriptors                                                                               |
| everything else            |     ~2,000 | descriptors, fonts, the Shift-JIS table, scalars                                                  |
| **Total**                  | **~80 KB** |                                                                                                   |

#### Allocated once at boot

| Consumer                                                                                                                                                                            |   Bytes |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------: |
| ESP-IDF's own tasks (main 8,704, TCP/IP 3,584, esp_timer 4,096, event 2,816, IPC ×2 2,560, idle ×2 3,072, FreeRTOS timer 2,048, WiFi ~3,584, mDNS 4,096, httpd 8,192) plus ~13 TCBs | ~53,000 |
| Firmware tasks: journal log 4,096, scan 8,192, net 8,192, health 6,144, status 4,096. The journal drain's 8,192 joins them only when`DEBUG` is set                                  |  30,720 |
| C tasks:`beamer_msc` 6,144, `beamer_wbc` 4,096, plus TCBs and six semaphores                                                                                                        | ~12,400 |
| WiFi pinned RX buffers, 10 × ~1,600                                                                                                                                                 | ~16,000 |
| WiFi RX management buffers, 5 × ~500                                                                                                                                                |  ~2,500 |
| mDNS steady state                                                                                                                                                                   |  ~7,000 |
| NVS page cache                                                                                                                                                                      |  ~3,000 |
| The read window's FatFs registration                                                                                                                                                |   2,220 |
| The rendered reset census, two short lines held for the boot                                                                                                                        |    ~250 |

#### Dynamically allocated

| Allocator                                                                                   |                       Block | Rate                                              |
| ------------------------------------------------------------------------------------------- | --------------------------: | ------------------------------------------------- |
| lwIP pcbs, pbufs and tcp_segs — built with`MEMP_MEM_MALLOC=1`, so there are no static pools |            200 B – 23,040 B | continuous, per connection and per packet         |
| A replay download's TCP send queue —`CONFIG_LWIP_TCP_SND_BUF_DEFAULT`, 16 × MSS             | <=23,040 B in 1,440 B pbufs | held for the length of one`GET /SLIPPI/<file>`    |
| WiFi dynamic TX buffers, 32 cap                                                             |                    ~1,600 B | per transmit burst                                |
| mDNS per-packet buffer,`MALLOC_CAP_INTERNAL`                                                |                   <=1,460 B | every multicast on the network                    |
| FatFs long-filename working buffer                                                          |                       512 B | every`f_open`, `f_opendir`, `f_readdir`, `f_stat` |
| `vfs_fat_dir_t` for a directory walk                                                        |                      ~330 B | per listing                                       |
| The scan tick's name-hash vector - the last walk's and the one being built are both live    |  <=16,384 B contiguous each | two per listing                                   |
| An`OsString` per directory entry                                                            |                       ~40 B | × file count, per listing                         |
| `GET /status` — the JSON body plus the error array                                          |    <=7,000 B in many pieces | per request                                       |

##### Hard limits

|                         | Limit                                    | Set by                                                                                                                                              |
| ----------------------- | ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| FatFs volumes           | 2                                        | `CONFIG_FATFS_VOLUME_COUNT` — one is held for the boot by the read window                                                                           |
| FatFs sector size       | 512 B                                    | `CONFIG_WL_SECTOR_SIZE_512`; it sizes `FATFS.win[]` and every `FIL.buf[]`, and at 4096 the mount context is 21 KB instead of 2.2 KB                 |
| Open files per mount    | 2                                        | `MAX_FILES` in `storage/fat.rs`                                                                                                                     |
| Concurrent HTTP sockets | 4                                        | `esp_http_server`, 192 B of state each. It is one task, so handlers run one at a time and only one send queue is ever full                          |
| TCP send window         | 23,040 B                                 | `CONFIG_LWIP_TCP_SND_BUF_DEFAULT`; window/RTT is the download ceiling, so this is what sets it. `CONFIG_LWIP_TCP_WND_DEFAULT` stays at 5,760        |
| Write-back cache        | 64 sectors                               | `WBC_SECTORS`; `high water` and `stalls` in `LOGS/debug.txt` say whether it is enough                                                               |
| Replays served          | 16                                       | `NUM-REPLAYS-SERVED` ceiling                                                                                                                        |
| Replays counted         | `REPLAY-CAP`, default 512, ceiling 2,048 | `replay_count` saturates at `replay_cap`, both reported in `GET /status`                                                                            |
| Error text kept         | 2,048 B per store                        | then truncated                                                                                                                                      |
| Captured log kept       | 4,096 B                                  | oldest lines dropped                                                                                                                                |
| Journal partition       | 64 KB                                    | `jrnl`, separate from `nvs` so diagnostics cannot exhaust a station's durable state; the reset census adds one 37-byte key, rewritten once per boot |

### Releasing

Pushing a `v*` tag builds the firmware in GitHub Actions and leaves a draft release carrying `beamer.bin`.

```bash
git tag v1.2.3 && git push origin v1.2.3
```

This creates a draft, not a new release - go manually publish the draft in Github!

### Details

#### Basics

`std` Rust on ESP-IDF, via `esp-idf-hal` / `esp-idf-svc` / `esp-idf-sys`. ESP-IDF provides a newlib environment, so this is real `std` — ordinary error handling, `serde_json` for the report writers — plus direct access to the C components underneath: TinyUSB for Mass Storage, FatFs for the volume, `esp_http_server`, `esp_wifi`, `mdns`, `esp_lcd`.

The TinyUSB callbacks need a small `unsafe` FFI shim — USB descriptors and the `tud_msc_*` entry points, in `components/beamer_msc`. That is the only raw FFI in the project and it should stay that way.

#### Storage odds and ends

##### Card size

Format the replay partition (the first FAT32 partition) to about 4 GB with 4 KB clusters. If you don't do this, shit breaks. The host sees exactly as much disk as the partition describes.

The volume is only written during two windows, and everything else that touches it is read-only:

1. `PrepareForBind`, before the USB bind, when no host can possibly hold the medium.
2. After the host ejects.

Everything read outside those windows invalidates the FatFs cache.

Durable state — the identity, the last-good WiFi config, and the log ring persisted on error — lives in NVS.

#### Fleet determinism

A station's behaviour must be a function of its config file and nothing else. This makes management easy. Don't try and be nice and forgive bad conflicts - fleets will drift and debugging will

#### The RAM write-back cache

32 KB of internal SRAM sits between the host and the card — 41 KB once the flush staging buffer and the slot metadata are counted, which is what it costs the heap ceiling. A write lands in RAM and returns; a task behind it moves sectors out. That shields Slippi Nintendont from the the SD card stalling ()the spec permits a card to hold busy for 250 ms on a single-block write).

While a sector is dirty, the host believes a write landed that has not, and pulling the cable loses it. This is the reason for the `BUSY` state that tells TO not to unplug sometimes, including during a live game.

Errors quiesce the cache before the LED turns red by switching to write-through - that way an Error'd beamer can be safely pulled from a Wii.

#### FreeRTOS tasks

Keyed by the task's real name, because that is what a panic backtrace, a coredump listing and `uxTaskGetSystemState` print — not the prose label.

| Task                                                                               | Priority               | Core | Created at                               |
| ---------------------------------------------------------------------------------- | ---------------------- | ---- | ---------------------------------------- |
| `beamer_msc` — the TinyUSB device loop, and the SCSI callbacks that reach the card | 22                     | 1    | `components/beamer_msc/beamer_msc.c:628` |
| `beamer_wbc` — the write-back cache flush                                          | 10                     | 1    | `components/beamer_msc/beamer_wbc.c:235` |
| `httpd` — `esp_http_server`. ESP-IDF creates it; the firmware only configures it   | 5                      | 0    | `src/net/http.rs:28`                     |
| `net` — WiFi association, then HTTP and mDNS bring-up                              | 4                      | 0    | `src/net/mod.rs:120`                     |
| `scan` — the 10 s tick                                                             | 4                      | 0    | `src/scan.rs:89`                         |
| `status` — the LED and the panel                                                   | 3                      | 0    | `src/status/mod.rs:182`                  |
| `health` — the network health tick                                                 | 2                      | 0    | `src/net/check.rs:35`                    |
| `journal` — journal, when in debug mode                                            | 1                      | 1    | `src/journal.rs:1037`                    |
| `jrnl-log` — the `esp_log` capture drain, and the UART write                       | 1                      | 1    | `src/journal.rs:980`                     |
| `main` — runs `boot::run` and returns                                              | 1, the ESP-IDF default | 0    | `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0`     |

#### Station Identity

Each board assigns its own, at first boot: a UUIDv5 over the chip's factory-programmed base MAC (hashed as lowercase colonless hex) against the same fixed project namespace upstream uses. The USB serial descriptor becomes `BEAMER-<uuid>`. Reflashing a board keeps the same identity, because the MAC is in eFuse and the firmware never writes it.

The hostname comes from `STATION-NAME`, not from the UUID, and is re-derived on every boot: renaming a station in `config.txt` renames it on the network at the next power cycle. The name is slugged — lowercased, every run of anything outside `[a-z0-9]` collapsed to one hyphen, trimmed of leading and trailing hyphens, cut to 56 characters — and `beamer-` is prefixed to it. `Stream Station 2` becomes `beamer-stream-station-2`. As a result, names that slug to nothing fall back to the ID**.** `STATION-NAME=拉拉` is a perfectly good name for `GET /status`, and it carries there in full — but there is no hostname in it, so that station stays `beamer-<uuid>`.

Nothing enforces uniqueness. Two stations named `Setup 2` claim the same hostname, the same way two hosts on any network would; the UUID underneath them stays distinct, and that is what `GET /status` and the replay index report.
