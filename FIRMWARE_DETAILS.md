## Beamer firmware

### Build

Build requires the esp-rs toolchain and ESP-IDF. If you dont have them installed:

```bash
cargo install espup espflash ldproxy
espup install
```

`espup` installs the xtensa Rust fork as a rustup toolchain and **writes `~/export-esp.sh`, which has to be sourced in each new shell**. `ldproxy` is needed for linking.

To build:

```bash
source ~/export-esp.sh
cargo build --release
```

To build and flash onto a beamer in download mode:

```bash
source ~/export-esp.sh
cargo run --release
```

**Editing `components/` does not, on its own, force cargo to rebuild the C code. If you edit C code your first rebuild will fail.`build.rs` deals with this this for you - just run the build again and it will work the second time.**

### Status readout

**Blinking means something is happening - DO NOT UNPLUG.**

A warning icon will appear and the LED will go amber if something is wrong but it's application recoverable or ignorable (drive needs to be reset, wifi is a little too weak for safety, etc). If something is wrong but it needs irl TO attention (SD card misformatted, wrong wifi, etc) the LED will go red and the whole screen will show an error instead.

### Error labels

| Label           | What happened                                                                                                                                                                                                                                                                                                                         |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `NO ID`         | The board's factory MAC is unset or all zeroes.<br /><br />**This is a board-side hardware issue.**                                                                                                                                                                                                                                   |
| `NO SD CARD`    | The card slot came up empty or no card responded.<br /><br />**This is probably a microSD card hardware issue.**                                                                                                                                                                                                                      |
| `SD UNREADABLE` | A card is present but its filesystem will not mount.<br /><br />**This is probably a microSD card hardware issue.**                                                                                                                                                                                                                   |
| `WRONG FORMAT`  | A card is readable but has no FAT32 partition or a first FAT32 partition bigger than 4 GB.<br /><br />**Reformat the microSD card**.                                                                                                                                                                                                  |
| `NO CONFIG`     | `CONFIG/config.txt` could not be read. <br /><br />**Fix config.txt.**                                                                                                                                                                                                                                                                |
| `BAD CONFIG`    | The config file was read and rejected.<br /><br />**Fix config.txt.**                                                                                                                                                                                                                                                                 |
| `NO USB`        | The USB stack would not start.<br /><br />**This is a board-side hardware issue.**                                                                                                                                                                                                                                                    |
| `RADIO FAILURE` | The ESP32 radio would not start.<br /><br />**This is a board-side hardware issue.**                                                                                                                                                                                                                                                  |
| `WIFI ISSUE`    | The wifi won't connect. Shows fifteen seconds after boot.** Usually this just means you put the wrong WiFI password. Fix config.txt. If that wasn't the issue, move the router closer to this setup. The beamer antenna is not as strong as the ones in your phone and laptop!**                                                      |
| `WIFI TOO FULL` | The wifi connected but didn't issue an IP address - usually this means there are too many devices connected to the router.<br /><br />**Get your own router - see [choosing a router](#choosing-a-router). Sorry, the venue's router isn't cutting it for your tournament anymore. Tournament too big - good problems to have, huh?** |
| `NO HTTP`       | Nothing is being served over HTTP<br /><br />**DM me @jenpissgirl on Discord...**                                                                                                                                                                                                                                                     |
| `NO MDNS`       | mDNS is not being offered<br /><br />**DM me @jenpissgirl on Discord...**                                                                                                                                                                                                                                                             |
| `OUT OF MEMORY` | Beamer ran out of Memory<br /><br />**DM me @jenpissgirl on Discord...**                                                                                                                                                                                                                                                              |
| `CRASHED`       | The firmware panicked somewhere.<br /><br />**DM me @jenpissgirl on Discord...**                                                                                                                                                                                                                                                      |

### Warning labels

| Label           | What is off                                                                                                                                                                                                                                                                                                        |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `DRIVE FAILING` | The card has stopped answering reads. Replays are still recorded, but not counted or served.<br /><br />**Replace your microSD card - it's reached the end of its life. This should be temporary (it turns into an error if it interrupts beamer operations). **                                                   |
| `DRIVE FULL`    | `REPLAY-CAP` replays are on the card. New ones are no longer served.<br /><br />**Reset drive.**                                                                                                                                                                                                                   |
| `NO WII`        | Nothing has read this drive in fifteen seconds. Usually this just means your beamer is plugged into a charger, a dead port, or a linux box that never mounted it.<br /><br />**If this beamer is plugged into a Wii: bad USB port.<br /> Otherwise ignore.**                                                       |
| `SLP MISFORMAT` | A replay on the card will not parse. It is counted but never served; the station is otherwise fine.<br /><br />**Probably either a Slippi Nintendont software issue or Wii USB port hardware issue. <br />Ignore it once or twice - if it keeps coming up, try a different port.**                                 |
| `DRIVE FILLING` | The card is past 75% of`REPLAY-CAP`. Delete replays before it stops serving new ones.<br /><br />**Reset drive.**                                                                                                                                                                                                  |
| `LOW MEMORY`    | Not enough heap to take another connection. Replays are refused with`503` until there's space.<br /><br />**Beamer is getting hammered pretty hard.<br />Ignore it once or twice - if it keeps happening, too many people are connecting to your beamer. Consider using sharded networking instead of connected.** |

### FAT cache

USB hosts typically cache FAT directories pretty aggressively. Writing a directory entry straight to the card while a host is holding the USB will almost always lead to desynchronized states between the host and the actual drive - this causes all sorts of issues with writing over used sectors, unlinked files, etc. **As a rule, once a host has connected the FAT volume is read-only to beamer firmware**. The firmware writes to the volume in exactly two windows, both when no host holds the medium:

1. Before the USB bind at boot, which is why the config is read early rather than when it is first needed.
2. After the host ejects, which is a clean SCSI media-change the firmware is told about.

Everything else is strictly read-only and re-reads the FAT rather than caching any directory entries.

### Memory

**Dynamic allocation follows a strict rule: never allocate a block larger than 512B once the station is `Running` unless the station can stay fully operational if that allocation fails.** Prefer moving large allocations off the heap wherever possible. In Rust, this usually means `static` or `heapless` - in C it usually means a file-scope `static`. Exceptions can be made in debug mode(`journal`'s log tail is one).

#### Allocated statically at link time

| Consumer                     |       Bytes | Description                                                                                                                                                                                              |
| ---------------------------- | ----------: | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ESP-IDF and its libraries    |      43,540 | WiFi/PHY/supplicant 12,314, FreeRTOS 4,732 (mostly`port_IntStack`), the `esp-idf-*` crates 1,843, Rust `std`/`core` 1,643, lwIP 335, a 16,474 tail, and 6,199 of padding                                 |
| `beamer_wbc.c` `s_data`      |      32,768 | the write-back cache itself:`WBC_SECTORS=64`sectors of 512 B each                                                                                                                                        |
| `beamer_gz.c` `s_arena`      |      15,360 | zlib allocations over a 1 KB window                                                                                                                                                                      |
| `beamer_wbc.c` `s_staging`   |       8,192 | write back cache flush space                                                                                                                                                                             |
| `beamer_msc.c` `s_ring`      |       8,192 | 512 transfer timings - used to track read / write time on SD cards                                                                                                                                       |
| `beamer_log.c` `s_ring`      |       8,192 | the`esp_log` capture that becomes `LOGS/debug_N.txt`, 4,096 B of it kept per boot with the oldest lines dropped.<br /><br />because this is allocated at link time, `DEBUG=false` does not give it back. |
| `lcd.rs` `SCRATCH`           |       8,192 | a 160×24 band of the led panel or a 64x64 boot animation frame                                                                                                                                           |
| `http.rs` `SCRATCH`          |       6,144 | a 2 KB read chunk off the card and a 4 KB block of compressed output                                                                                                                                     |
| `scan.rs` `seen` + `present` |       4,160 | `REPLAY-CAP` filename hashes and presence bitmap                                                                                                                                                         |
| `http.rs` `BODY_BUF`         |       4,096 | `GET /status` or `GET /SLIPPI/` body                                                                                                                                                                     |
| `publish.rs` `index_buf`     |       2,560 | replay index json                                                                                                                                                                                        |
| `errors.rs` `STORE`          |       7,210 | the session, late and previous error blobs at`CAP` each, plus the summary being constructed                                                                                                              |
| `reload.rs` `SCRATCH`        |       4,096 | `config.txt`                                                                                                                                                                                             |
| `scan.rs` `FAST.game`        |       1,024 | the published game blob                                                                                                                                                                                  |
| `journal.rs` `ENCODE_BUF`    |         861 | the NVS summary blob                                                                                                                                                                                     |
| TinyUSB`_mscd_epbuf`         |       2,048 | `CFG_TUD_MSC_EP_BUFSIZE`(TinyUSB endpoint data buffer)                                                                                                                                                   |
| `beamer_msc.c` `s_stack`     |       6,144 | `beamer_msc` task stack                                                                                                                                                                                  |
| `beamer_wbc.c` `s_stack`     |       4,096 | `beamer_wbc` flush task stack                                                                                                                                                                            |
| `beamer_wbc.c` `s_meta`      |         768 | 64 slot descriptors                                                                                                                                                                                      |
| `volume.rs` `WIPE_BATCH`     |       2,048 | 8 replay names for`POST /reset-beamer`, so the wipe does not build a list of every replay on the heap                                                                                                    |
| everything else              |       4,029 |                                                                                                                                                                                                          |
| **Total**                    | **167 KiB** |                                                                                                                                                                                                          |

#### Allocated once at boot

| Consumer                                                                                                                                                                                                                     |        Bytes |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -----------: |
| ESP-IDF's tasks (main 8,704, TCP/IP 3,584, esp_timer 4,096, event 2,816, IPC ×2 2,560, idle ×2 3,072, FreeRTOS timer 2,048, WiFi ~3,584, mDNS 4,096, httpd 8,192) plus ~13 TCBs                                              |      ~53,000 |
| Firmware tasks: journal log 4,096, scan 8,192, net 8,192, status 4,096, transfer 6,144 (peaks at 3,352 B, and does not move with how well a replay compresses). The journal drain's 8,192 joins them only when`DEBUG` is set |       30,720 |
| WiFi (minus the`.bss` portion)                                                                                                                                                                                               |       21,950 |
| httpd's lwIP pools and loopback control socket                                                                                                                                                                               |        5,508 |
| The multicast socket                                                                                                                                                                                                         |       ~1,500 |
| mDNS (minus the`.bss` portion)                                                                                                                                                                                               |        1,932 |
| NVS page cache                                                                                                                                                                                                               |       ~3,000 |
| The read window's FatFs registration                                                                                                                                                                                         |        2,220 |
| The rendered reset census, two short lines held for the boot                                                                                                                                                                 |         ~250 |
| Journal drain task (only when`DEBUG=true`)                                                                                                                                                                                   |        8,192 |
| **Total**                                                                                                                                                                                                                    | **~125 KiB** |

#### Allocated by lwIP

| Consumer                                       |      Bytes |                                                                          |
| ---------------------------------------------- | ---------: | ------------------------------------------------------------------------ |
| One queued TCP segment                         |      1,536 | a`pbuf` of 16+56+1440 and a `tcp_seg` of 16, each +4 for TLSF            |
| One connection's send queue,`SND_BUF` 8,640    |      9,216 | 6 segments;`LWIP_NETIF_TX_SINGLE_PBUF` rounds every one up to a full MSS |
| **Both sockets,`max_open_sockets` = 2**        | **18,432** | what serving actually costs, since replay bytes fill every segment       |
| The WiFi driver's copy of each of those frames |     19,560 | ~1,630 B per frame,`MALLOC_CAP_INTERNAL\|DMA\|8BIT`, taken on demand     |
| **Peak in flight**                             | **37,992** | every byte in flight is buffered twice, once each side of the driver     |

#### Summary

|                                       |      Bytes |
| ------------------------------------- | ---------: |
| Total SRAM                            |    512 KiB |
| ...instruction cache and ROM reserved |     80 KiB |
| ...IRAM, the firmware's own code      |     94 KiB |
| ...allocated statically at link time  |    167 KiB |
| ...left for the heap                  |    171 KiB |
| Allocated once at boot,`DEBUG=false`  |   ~117 KiB |
| ...`DEBUG=true`                       |   ~125 KiB |
| Allocated by lwIP while serving       |   9-18 KiB |
| Free heap at rest                     |    ~44 KiB |
| Free heap while serving               | ~25-45 KiB |
| Largest free block at rest            |    ~31 KiB |
| Largest free block while serving      |   ~7.5 KiB |

### Firmware odds and ends

#### Card size

Format the replay partition (the first FAT32 partition) to about 4 GB with 4 KB clusters. If you don't do this, shit breaks.

#### Fleet determinism

A station's behaviour must be a function of its config file and nothing else.

#### The RAM write-back cache

32 KB of internal SRAM sits between the host and the card. Writes land in RAM and return immediately while a draining task moves those cached sectors onto the card. This shields hosts from the SD card stalls (which can honestly be quite frequent). While a sector is dirty, ejects can seriously mess up the state of the microSD card. This is why there are `BUSY` states to tell TOs not to unplug.

Errors quiesce the cache before the LED turns red by switching to write-through - that way an error'd beamer can be safely pulled without ejecting.

#### FreeRTOS tasks

I really make an effort to keep this table up to date - it's not trivially self documenting.

| Task                        | Priority               | Core | Created at                           |
| --------------------------- | ---------------------- | ---- | ------------------------------------ |
| `beamer_msc`                | 22                     | 1    | `components/beamer_msc/beamer_msc.c` |
| `beamer_wbc`                | 10                     | 1    | `components/beamer_msc/beamer_wbc.c` |
| `httpd` (`esp_http_server`) | 5                      | 0    | `src/net/http.rs`                    |
| `transfer`                  | 4                      | 0    | `src/net/transfer.rs`                |
| `net`                       | 4                      | 0    | `src/net/mod.rs`                     |
| `scan`                      | 4                      | 0    | `src/scan.rs`                        |
| `status`                    | 3                      | 0    | `src/status/mod.rs`                  |
| `journal` (DEBUG only)      | 1                      | 1    | `src/journal.rs`                     |
| `jrnl-log`                  | 1                      | 1    | `src/journal.rs`                     |
| `main`                      | 1, the ESP-IDF default | 0    | `CONFIG_ESP_MAIN_TASK_AFFINITY_CPU0` |

#### Station Identity

Each board assigns itself a `station_id` at first boot: a UUIDv5 over the chip's factory-programmed base MAC (hashed as lowercase colonless hex) against a fixed project namespace. Reflashing a board thus keeps the same `station_id`. The hostname is derived from the `station_id`, slugged and with `beamer-` prefixed.
