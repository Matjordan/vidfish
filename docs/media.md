# GStreamer prototype and deployment

The spike records a selected H.264/H.265 RTSP profile over TCP directly to a new
MKV file, without a video decoder or encoder in the recording pipeline. It is not
the multi-camera acquisition engine: auto-discovery, segmentation, reconnection,
recording-health monitoring, audio, and operator record controls remain later epics.

## Run the prototype

```sh
vidfish --media-check
vidfish --media-check --h265
vidfish --record rtsp://CAMERA/PROFILE --output NEW_FILE.mkv
vidfish --record rtsp://CAMERA/PROFILE --output NEW_FILE.mkv --seconds 30
vidfish --inspect NEW_FILE.mkv
vidfish --preview rtsp://CAMERA/PROFILE
vidfish --demo --desktop-seconds 10
```

Add `--h265` for an H.265 profile. Repeat `--preview` up to four times. The preview
process has separate connections and never owns the recording pipeline. Close or
restart the desktop while a recording process continues. Stop the recorder with
Ctrl+C; EOS drains the muxer with a 10-second finalization limit. A failure exits
nonzero and leaves partial media for recovery. Existing recording paths are refused.
Do not place credentials in command history. Secure credential binding is part of #11.

`--demo` includes four local 1080p encoders as well as decoders, so its CPU load is
not representative of real cameras. `--desktop-seconds` reports received and displayed
frame counts and fails if any selected preview never displays a frame. Render-driver
and selected decoder names are logged. `decodebin` selects available decoders by
GStreamer rank; hardware decoding depends on the device's drivers and installed
plugins. Override ranks only for a measured device configuration. RGB frames are
uploaded into egui textures for GPU rendering; zero-copy GPU video upload is not
implemented. Software decode/rendering remains the fallback.

## Required runtime

GStreamer 1.20 or later is required. Software fallback includes libav H.264/H.265
decoders. Core/base/good/bad/libav supply `rtspsrc`, RTP depayloaders, parsers,
`matroskamux`, `filesink`, `decodebin`, `videoconvert`, `appsink`, and media discovery.
Test fixtures additionally need ugly (`x264enc`), bad (`x265enc`), `jpegenc`, and
gst-rtsp-server. A missing element is reported by `--media-check` or the relevant
operation. No decoder is needed to record a supported profile, but previews and
media inspection require one. Preview resources are bounded to one appsink sample
and one latest RGB frame per camera (up to 4096x2160); only four previews are allowed.
Frame replacement drops stale preview frames rather than blocking recorders.

### Linux

Release executables are built on Ubuntu 22.04. On compatible Debian/Ubuntu systems,
install runtime libraries without Rust or development headers:

```sh
sudo apt-get update
sudo apt-get install libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  libxkbcommon0 libgl1
./launch-vidfish.sh
```

An X11/Wayland desktop and functioning OpenGL driver are required for the GUI.
Headless recording and inspection need no display. Developers additionally install
`libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `libgstrtspserver-1.0-dev`,
and `pkg-config`. UI smoke tests use Xvfb with software rendering; that is not a
Toughpad performance measurement.

### Windows

The ZIP carries GStreamer runtime DLLs, plugins, presets, scanner, and original
notices in `gstreamer/`, plus Microsoft's signed `vc_redist.x64.exe`. Install the
redistributable if it is absent, keep the package directory together, and run
`launch-vidfish.cmd`. No Rust toolchain or Visual Studio is needed. The GPU driver
must support OpenGL 3.3. Copying only `vidfish.exe` is not a supported deployment.
Redistributed GStreamer plugins retain their respective LGPL/GPL and other notices;
source releases and corresponding build tooling are available from
https://gstreamer.freedesktop.org/download/ and https://gitlab.freedesktop.org/gstreamer/cerbero.
Windows developers/CI install both MSVC runtime and devel SDKs via
`scripts/setup-gstreamer.ps1` (GStreamer 1.26.10). Mixed 32/64-bit runtimes are unsupported.

## Repeatable fixture tests

```sh
cargo test --locked --workspace --all-features
cargo run --locked -p vidfish-media --example camera-fixture --features fixtures
```

The camera fixture binds only loopback and prints RTSP, ONVIF SOAP, and unicast
WS-Discovery addresses. It produces H.264 1080p video plus a JPEG snapshot. SOAP
supports device information, capabilities, profiles, stream URI, and snapshot URI;
unsupported actions produce a SOAP fault. It does not simulate multicast networking,
authentication, or all vendor behavior. Tests also generate H.265 RTSP media and
inspect finalized codec/dimensions/duration.

`scripts/media-smoke.py` checks actual executable recording and inspection. With
`--desktop`, it kills and restarts a preview process and verifies the recorder still
finishes a playable 1080p recording. CI runs these workflows on Windows and Linux.

## Failure boundaries

Incorrect profiles, inaccessible cameras, missing codecs/plugins, write failures,
and invalid output paths are reported. Raw GStreamer error/debug strings are excluded
from application errors because they may contain URI credentials. A preview receiving
no frames remains visibly waiting; `--desktop-seconds` makes this a failing check.
Do not infer recording health from a live preview. Disk-full recovery, stream stalls,
split-boundary handling, and sustained 16-camera acquisition belong to #13–#16.

## Target performance acceptance

Epic #1 requires four simultaneous 1080p previews measured on the actual Toughpad,
with model, OS, CPU/GPU, camera codec/fps/bitrate, decoder names, render driver, CPU
utilization, GPU utilization, received/displayed frame rates, and memory consumption.
Cloud Xvfb results validate functionality only. Record target measurements separately;
do not mark #8 complete based on virtual display or simulated-source results.

On Windows, use `scripts/benchmark-toughpad.ps1` with exactly four real camera profile
URIs and the packaged binary. It records system/model/driver information, selected
decoders/renderer, per-slot frame counts, CPU and memory samples, and per-process GPU
engine counters when the driver exposes them. Results go to a new directory and
omit camera URIs. Add the actual profile codec/fps/bitrate to the measurement report.
If GPU counters are unavailable (including localized names or older WDDM drivers),
collect GPU utilization with a supported vendor tool; missing readings are not zero.
Before benchmarking the packaged executable directly, prepend its `gstreamer/bin`
to PATH and set `GST_PLUGIN_SYSTEM_PATH_1_0` to the bundled plugin directory.
