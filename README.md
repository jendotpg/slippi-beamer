# Slippi Beamer

STATUS: we have multiple brackets that have succesfully run on beamer! it's not quite at full release, though - there are still some bugs im tracking down before then :)

TODO:

- make scan beat transfer when theyre both waiting for RO_LOCK
- fix use-after-free thats breaking retry-after
- shrink the write-back-cache - slippi nintendont never even comes close to filling all 64 sectors. like... 24 is probably overkill.
- update ROUTERS.md

A [Beamer](https://github.com/jendotpg/slippi-beamer) is a microprocessor attached to a Wii over the USB port. The Beamer presents a disk image to the Wii as an ordinary USB flash drive. Slippi Nintendont writes .slp files to it believing it is a stick. The Beamer then serves those same replays over the tournament WiFi (or, for bigger tournaments, over a dedicated IoT access point).

For details on Beamer API, see [API.md](./API.md) . For details on this firmware, see [FIRMWARE_DETAILS.md](FIRMWARE_DETAILS.md). For suggested router setup, see [ROUTERS.md](ROUTERS.md).

## Running a tournament on Beamers

**WARNING: BEAMER IS CURRENTLY ONLY TESTED FOR TOURNAMENTS OF UP TO ~15 SETUPS. IF YOUR TOURNAMENT IS BIGGER THAN THAT AND YOU WANT TO RUN THIS, REACH OUT TO ME DIRECTLY!**

I have already scheduled a few tournaments bigger than this and know how I plan to run them, but it's certainly not confirmed to work. This will come with time :)

### Equipment

First, you'll need to buy+assemble+configure one Beamer for each Wii. See the steps below for guidance there.

You'll likely want to bring your own router (you can get an appropriate one for ~$40 max) as most venues will have routers that are too far away for the Beamers to work consistently, not to mention many having captive portals, limited DHCP pools, or device isolation. See [ROUTERS.md](ROUTERS.md) for guidance. Make sure to configure both the Beamers and your TO computer to connect to your router's network, not the venue's!

Finally, make sure you have some label for station numbers (I use table number stands like restaurants have).

### Set-up

Plug one Beamer into each Wii after booting into Melee (make sure you're on Slippi Nintendont 1.13.0 or later). Press the button on the Beamer until the number on the screen matches the station number. If you overshoot, holding the button counts backwards ;)

Open up the [Beamer fork of replay manager](https://github.com/jendotpg/replay-manager-for-slippi). Turn on auto-subscribe in settings. Before starting any games, erase the drive on every Beamer. You can do this later, but it won't work while someone is playing a game and so can be quite annoying to do later! If the drive fills up, replays stop coming.

### Reporting

Click the Beamer icon (it looks like a remote control) in the upper menu of Replay Manager. From here, you can see the status of all the beamers in the field - clicking one will download the last replays from that station so that you can report a set using the regular Replay Manager interface we all know and love.

## Buying a Beamer

| Item                        | Detail                                                                                                                                                                                        | Where I Source Them                                                                                                                    | Price |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| LilyGO T-Dongle-S3 with LCD | Get the variant with the screen - otherwise setting station number is VERY annoying.                                                                                                          | [www.amazon.com/dp/B0BK9162QY](https://www.amazon.com/dp/B0BK9162QY?lv=shuf&channelId=500&plpRedirect=mhFallback&th=1)                 | ~$15  |
| microSD card                | Any size from 4 GB up. Make sure to format the card to 4 GB FAT with 4 KB clusters.                                                                                                           | [www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793](https://www.digikey.com/en/products/detail/htsemi/HTF016G3U1/29285793) | ~$6   |
| Router                      | Only really needed if you have more than ~10 setups - otherwise, you can probably get away with venue wifi.<br /><br />One per section. Try to keep all setups within ~20 feet of the router. | see[ROUTERS.md](ROUTERS.md)                                                                                                            |       |

## Setting up a new Beamer

1. Format your microSD card - FAT32, first partition sized at 4GB (or smaller).
2. Insert the microSD card into the dongle. Note: The microSD slot is INSIDE the usb jack! Remove the dummy card that comes inside to insert the new one.
3. Hold the button on the side of the board dongle while you plug it into your laptop, then let go. Navigate to [the flashing page](https://jendotpg.github.io/slippi-beamer/) and press flash.
   1. "Leaving..." means its done - you don't have to wait any longer!
   2. This only works on Chrome, sorry. If you don't want to install Chrome, you can flash using `espup`or `cargo` - see [FIRMWARE_DETAILS.md](FIRMWARE_DETAILS.md)

4. Unplug and replug the dongle to leave download mode. The first boot derives the station identity and lays down `CONFIG/` and `LOGS/`
5. Fill in `CONFIG/config.txt` with SSID and Password.
   1. See [Configuring a station](#configuring-a-beamer) for more details on this file.
   2. Watch the screen/LED. If it goes green and shows Station 1 your Beamer is working and ready to go! Otherwise, you probably entered the wifi wrong. This will look like about fifteen seconds of booting followed by a screen that says "WIFI ISSUE".

## Configuring a Beamer

Station number is set with the button on the beamer - clicking goes up and, if you overshoot, holding the button will go down. Other configuration (most importantly wifi info) is set by editing `CONFIG/config.txt`. Keys are case-insensitive, blank lines and `#` comments are ignored, and values may be quoted.

| Key                  | Default | What it does                                                                                                                                              |
| -------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SSID`               | blank   | The network to join.                                                                                                                                      |
| `PASSWORD`           | blank   | 8–63 characters. Blank means an open network.                                                                                                             |
| `COUNTRY`            | `US`    | Two-letter regulatory domain:`US`, `CA`, `JP`, `GB`...                                                                                                    |
| `HIDDEN`             | `false` | Whether the network broadcasts its name.                                                                                                                  |
| `NUM-REPLAYS-SERVED` | `10`    | How many of the newest replays the station hands out over HTTP. 1 to 16.                                                                                  |
| `REPLAY-CAP`         | `512`   | How many replays the station counts on the card before it stops counting. 1 to 512. Past 75% it warns; at the cap it warns and stops serving new replays. |
| `LED-BRIGHTNESS`     | `20`    | The status LED brightness, 0 to 100 percent.                                                                                                              |
| `FLIP-SCREEN`        | `false` | Whether the screen starts upside down. If you stand your Wii up, you probably want this.                                                                  |
| `DEBUG`              | `false` | Debug mode. Don't use this unless you know what you're doing.                                                                                             |
