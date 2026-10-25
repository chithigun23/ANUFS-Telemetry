> Written on 2026-10-26, before the app existed. `../README.md` is the current state of the app. What is still useful here is the research: what the Quectel documents say about raw observations and the moving-base wiring, the UART path on the board, and the Geoscience Australia base stations near Canberra and The Bend.

# Android app plan: offload, view and PPK the telemetry logs

Written 2026-10-26 against `origin/main` (5c7ea0d). Phone: Galaxy S22 Ultra, OTG microSD reader (not bought yet). Nothing here has been run on the board or the phone.

## Goal

1. Plug a log card into the phone and copy the sessions off it.
2. Show the telemetry: track map, speed and IMU/CAN channels, laps.
3. Post-process the GNSS into a PPK track using Geoscience Australia (GA) base-station data.

## Facts that shape the design

| Fact | Source | Consequence |
|---|---|---|
| Cards are raw 512-byte CRC blocks, not FAT. A prepared card shows as unformatted. | `software/README.md`, `tel-logfmt` | Android will not mount it. The app talks to the reader over the USB Host API (SCSI bulk-only) and reads blocks itself. Never tap Android's "format" prompt. |
| Both slots mirror the same log. | `software/README.md` | The phone only needs one card. |
| The log holds `Sample` (NMEA-derived), SBG signals, raw CAN, events. No raw GNSS observations. | `tel-logfmt/src/records.rs` | PPK is impossible until the firmware also logs raw observations. |
| `tel-decode` reads blocks through a `BlockSource` trait; log and CAN crates are `no_std`. | `tools/tel-decode/src/source.rs` | Reuse them in the app via Rust + JNI instead of rewriting the parser. Add a USB-backed `BlockSource`. |

## GNSS raw observations: what the Quectel documents say

Sources: LC29H Protocol Specification V1.5, LC29H(EA) Moving Base Application Note V1.1, LC29H Module Specification V2.0, Hardware Design V1.4 (from `Downloads/q (1).zip`). Page references are in the extracted PDFs, not kept in the repo.

- **Position rate is 10 Hz; raw data rate is 1 Hz.** The EA's default fix interval is 100 ms. The module specification lists "Raw Data Update Rate: GNSS 1 Hz" for every variant. Above 1 Hz only RMC and GGA come out at the fast rate (GSA/GSV stay at 1 Hz, other NMEA is off). So the log can have 10 Hz position but PPK can only get 1 Hz observations (unless a bench test shows MSM faster than the datasheet says).
- `$PAIR432,<0|1>` enables RTCM3 MSM4 (0) or MSM7 (1); `$PAIR434,1` adds 1005; `$PAIR436,1` adds the ephemeris messages 1019, 1020, 1042, 1044, 1046. So the card can carry the broadcast ephemeris.
- **Moving-base wiring (what this board does).** The base (U10) talks to the MCU on its UART1. Its UART2 carries local corrections to the rover (U13) UART1, and the rover's heading comes back on the same pair. The note says no special configuration is needed: the modules identify themselves. The MCU gets NMEA plus `$PQTMTAR` from the base. So U10 is not in `$PQTMCFGRCVRMODE` base mode. If it were, NMEA would be switched off by default.
- **Port routing for RTCM is not documented for the EA.** `$PQTMCFGPROT` (per-port protocol selection) is only on the AA and AL parts. `$PAIR432` is documented without a port. Bench test needed.
- Baud: messages can be lost below 460800 baud at 10 Hz (spec note). The firmware already uses 460800.
- Baseline: Quectel quotes 15 km as the limit for guaranteed RTK accuracy. Its datasheet RTK figure assumes the base is within 1 km.

Open questions for the bench (power U10 alone, PC on UART1):
1. With the module in its moving-base default, does `$PAIR432,1` put MSM7 on UART1 next to NMEA, or only on UART2?
2. What is the real MSM rate: 1 Hz as the datasheet says, or the fix rate?
3. Does `$PAIR432,1` persist across `$PQTMSAVEPAR` and a power cycle (one forum report says it reverts to MSM4)?
4. Does enabling MSM disturb `$PQTMTAR` or the 10 Hz RMC/GGA stream?

## UART bandwidth: hardware check

Traced from `telemetry.kicad_pcb` pad nets:

- MCU `TXD1_GNSS` (U6 pin 15) and `RXD1_GNSS` (pin 14) go to U12 (TXS0104E) B side (pins 11, 12). U12 A side (pins 4, 3) goes to U10 `TXD1`/`RXD1` (pins 20, 21).
- U10 pins 18/19 (`CORRECTION`/`HEADING`, UART2) connect only to U13 pins 21/20. There is no MCU connection and no test point on those nets, so anything U10 sends on UART2 is invisible to the MCU.
- Verdict: the board path is not the limit. U12 is a level shifter rated in the Mbps range (from the datasheet family, not re-read), against 0.46 Mbaud. The limits are the module's output rate and the firmware's UART4 DMA ring buffer.
- Estimate (mine, unmeasured): MSM7 for about 30 satellites on L1+L5 is roughly 0.9 KB per epoch, so about 1 KB/s at the 1 Hz raw rate, a small fraction of the 46 KB/s that 460800 baud carries alongside the 10 Hz RMC/GGA.
- If the bench test shows RTCM only on UART2 (the moving-base note suggests this is where U10 sends corrections): the only fixes are a bodge wire from the `CORRECTION` net to a spare MCU UART (U6 has no obvious spare UART RX pin in the current net list; needs a look at the layout) or a respin.

## Base-station data (Geoscience Australia)

- Archive: `https://data.gnss.ga.gov.au/api/rinexFiles?stationId=…&startDate=…&endDate=…&filePeriod=15M&fileType=obs&rinexVersion=3`. Follows a redirect to `/api/v1/…`. Returns JSON with pre-signed S3 `fileLocation` URLs. Anonymous. Daily/hourly files are 30 s; `15M` files are 1 s, observation only, from a subset of stations. Files are `.crx.gz` (Hatanaka), so the app needs a CRX2RNX step. Navigation data is in the hourly/daily files.
- Station list with coordinates: the NTRIP sourcetable at `http://ntrip.data.gnss.ga.gov.au:2101/` (907 sites). No login needed to read it.
- Checked 2026-10-20 (one day only):

| Place | Station | Distance | 1 s in archive? |
|---|---|---|---|
| ANU Canberra | SYM1 | 8 km | yes |
| | STR1, STR2, STR3 | 10.6 km | yes |
| | CNBN | not computed | yes |
| | TID1 | 18 km | yes |
| | PRCE, GNGN, CNWD, CHSM | 10–16 km | no (30 s only) |
| The Bend (Tailem Bend) | TIBD | about 1 km from the town | **no, 30 s only** |
| | WTHL | 67 km | yes |
| | PTSV | 90 km | yes |

- STR1 (Septentrio PolaRx5) and TIBD (Trimble NetR9) both track L5 / E5a / B2a (headers read), so dual-frequency double differences with the LC29H are possible.
- The Bend: TIBD is the right station but the archive only has 30 s. RTKLIB needs base epochs at the rover epoch rate, and linear interpolation of 30 s base observations is not a sound fix. Options, best first:
  1. Own base: a second LC29H on a tripod at the track, logging MSM7 to its own card. Same firmware record, same app.
  2. Record TIBD's live 1 Hz RTCM stream on the phone during the session (free GA NTRIP account, needs mobile signal at the track; whether TIBD is open to anonymous accounts is unverified).
  3. Use WTHL/PTSV (67/90 km): expect float, not fixed.
- The 1 s files appear a couple of minutes after each quarter hour. Offline processing needs internet at some point.

## Architecture

- Kotlin + Jetpack Compose UI.
- USB: `UsbManager` + bulk-only SCSI (READ(10), READ CAPACITY). Small, read-only. libaums is a reference for the transport, but it only understands FAT32.
- Log core: Rust (`tel-logfmt`, `tel-can`, `tel-can-sbg`, `tel-can-motec`, scan logic from `tel-decode`) built with `cargo-ndk`, bound with UniFFI.
- PPK: RTKLIB demo5 `rnx2rtkp`/`convbin` built with the NDK behind a thin JNI layer. Existing Android RTKLIB ports are real-time `rtknavi` ports and old.
- Storage: copy each session to app storage as a verified image slice, keep the raw blocks, derive everything else.
- Map: MapLibre or osmdroid with cached tiles (no signal at the track).

## Stages

| Stage | Work | Needs |
|---|---|---|
| 0a | Card test: prepared card in the S22 via OTG reader. Does Samsung leave it alone? Can the app claim it? | reader, phone |
| 0b | Bench test of the four open questions above | board bring-up |
| 1 | Open a card image file: session list, decode, CSV export. USB reader added after 0a. | nothing (sample images in `telemetry-app/sample-data`) |
| 2 | Viewer: map, channels, CAN signals, laps | 1 |
| 3 | Firmware: raw-GNSS record kind; log RTCM bytes in chunks with the UTC timestamp, enable `$PAIR432`/`$PAIR436` in the configure sequence | 0b |
| 4 | PPK: RTCM to RINEX, fetch base + nav, RTKLIB, compare with the onboard fix | 3, base data |

## Things that can go wrong

- PPK positions are 1 Hz. Plan: compute the PPK-minus-onboard offset at 1 Hz, interpolate it, and apply it to the 10 Hz onboard track (works if the onboard error varies slowly; to be checked on real data).
- Samsung may keep prompting to format the unrecognised card. A small FAT partition in front of the log region might help, but `tel-decode prepare --start-lba` would have to be tested for that.
- Unconfirmed: Quectel's RTCM port routing; whether TIBD live data is open to a free account.
