# ANUFS Telemetry, Android viewer

Opens a raw image of a telemetry card and shows it: track on a map, laps against a start/finish line, IMU, GNSS and CAN charts, with the SBG Ellipse as a second, independent source. Exports GPX and CSV. Kotlin and Jetpack Compose on top; the log is read by the same Rust code as the PC tool (`tools/tel-decode`) through `tools/tel-jni`.

Status: builds, 32 Kotlin and the Rust tests pass, runs on a Galaxy S22 Ultra. It reads image files only; reading the card itself over USB is not done yet. Nothing has been checked against real logs from the board.

## Build

Needs the Rust target and cargo-ndk once, plus an Android NDK:

```
rustup target add aarch64-linux-android
cargo install cargo-ndk
export ANDROID_NDK_HOME=/path/to/android-ndk-r29     # build-rust.sh defaults to ~/Android/ndk-telemetry/android-ndk-r29
```

Then, with Android Studio's JDK and the SDK (platform 36, build-tools 36.1.0):

```
./gradlew :app:assembleDebug          # runs build-rust.sh first; APK in app/build/outputs/apk/debug/
./gradlew :app:testDebugUnitTest      # Kotlin tests
```

`local.properties` (not committed) must hold `sdk.dir=...`. The library versions are the newest that work with AGP 8.13 and compileSdk 36; newer ones need AGP 9 and compileSdk 37.

## Try it without the board

```
cd ..                                                   # software/
TEL_DEMO_IMAGE=../sample-data/demo_laps.img cargo test -p tel-decode --test demo_card
adb -s <phone> push ../sample-data/demo_laps.img /sdcard/Download/
```

`demo_laps.img` is a made-up three-lap drive with Ellipse signals and invented MoTeC frames. `TEL_DUMP_IMAGES=<dir> cargo test -p tel-decode --test pipeline` writes the two cards of the system test (one dies half way).

## Debug builds on a phone

A debug build accepts an intent that opens an image by path and logs the map's zoom under the `TrackMap` tag:

```
adb -s <phone> push demo_laps.img /data/local/tmp/
adb -s <phone> shell run-as au.anufs.telemetry cp /data/local/tmp/demo_laps.img files/
adb -s <phone> shell am start -n au.anufs.telemetry/.MainActivity \
    --es open_path /data/data/au.anufs.telemetry/files/demo_laps.img --ei open_session 1
```

`app/src/androidTest/.../MapPinchTest.kt` drives the map with real two-finger gestures and checks the zoom. Build it with `:app:assembleDebugAndroidTest`, install both APKs, then
`adb -s <phone> shell am instrument -w -e class au.anufs.telemetry.MapPinchTest au.anufs.telemetry.test/androidx.test.runner.AndroidJUnitRunner`.
Always pass `-s`: more than one device is often attached.

## Layout

| Path | What |
|---|---|
| `app/src/main/java/au/anufs/telemetry/core` | Pure Kotlin: scan JSON, samples CSV, signals, Ellipse view, settings, GPX. Unit tested on the JVM. |
| `app/src/main/java/au/anufs/telemetry/ui` | Compose screens, charts, map, settings. |
| `../tools/tel-jni` | Rust side of the bridge: `scan` and `export`, JSON in and out, errors as `{"error": ...}`. |
| `build-rust.sh` | cargo-ndk build into `app/src/main/jniLibs` (git-ignored). |

## Fused track

The Source chips offer **Fused** (the default), Onboard, Ellipse and Both. Fused is a best-estimate position made from the board's receiver and, when the session has one, the Ellipse: a Kalman filter per axis (east, north) with position and Doppler-velocity measurements from every source, then a backwards smoothing pass (`core/Fusion.kt`). Velocity is what smooths the track; Doppler speed is much more accurate than the difference of two positions. A position with no measurement within 3 s either side is left out, so a lost fix is not bridged with a straight line. With only the onboard receiver it is the onboard track smoothed with its own Doppler speed.

Laps and the "GPX track, fused" export use the fused track when Fused is chosen. The Summary tab collates the headline numbers, how each source did, how closely the two receivers agree, and the filter's own spread.

Assumed errors (Settings, "Fused track", can lean on one side): 1 m for a single-band receiver (scaled by HDOP), 3 cm and 30 cm for RTK fixed and float, 0.1 m/s for Doppler. They are guesses until real logs say otherwise. The filter assumes the receivers' errors are independent; they share satellites, so its spread is a lower bound, and the Summary leads with the measured gap between the receivers instead. The two IMUs are not fused (their axes differ) and the IMU does not yet feed the filter.

## Laps

The start/finish line is set on the Map tab (Start/finish menu) or the Laps tab, two ways:

- **Draw it** across the track with the S Pen. Fingers keep panning and zooming; the switch in the banner lets a finger draw on a phone without a pen.
- **Set it at the cursor**: scrub the cursor to where the car is on the line; the line is made square to the direction of travel, 40 m wide.

Crossings are found by intersecting each step between two fixes with the line (the time is interpolated inside the step), in the line's direction only, and at least "Shortest lap" (Settings) apart. A hand-drawn line takes the direction most crossings go. Lines are remembered; a later session whose track crosses a saved line gets its laps without asking. Laps follow the source chosen (Fused uses the fused track, Ellipse its own, Onboard and Both the board's receiver). The crossings show as dashed lines on every chart.

`LapsOnDeviceTest` drives this on a phone with a synthetic stylus stream, checks a finger does not draw, and sets a line at the cursor.

## Known gaps

- Reads image files, not the card over USB.
- The Ellipse's `STATUS_03.SOLUTION` words are decoded from memory of SBG's documentation (`core/Ellipse.kt`); check against the Ellipse-D manual.
- Charts use the 10 Hz sample-and-held Ellipse values; the full-rate raw CAN lane is not decoded yet.
- No PPK or base-station download yet.
- The fusion's error figures are assumptions, and it has only seen simulated data.
- Laps use the 10 Hz samples, so a crossing is interpolated between 0.1 s fixes; the line is straight and fixed in size (40 m) when set at the cursor.
- Drawing with a real S Pen has only been tried with injected stylus events.
