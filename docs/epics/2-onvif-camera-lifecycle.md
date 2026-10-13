# Epic #2: ONVIF discovery and camera lifecycle

Builds on the merged Rust foundation in epic #1. Implementation is in progress.

Implemented: bounded WS-Discovery probes with explicit IPv4 adapter selection,
correlated response collection, ONVIF device/Media1 profile resolution, and CLI
manual ONVIF/RTSP fallback. See [camera instructions](../cameras.md). Authentication,
persistent camera inventory, and continuous lifecycle management remain outstanding.

- [ ] #10: WS-Discovery with interface selection, ONVIF profiles, RTSP URI resolution, and manual fallback.
- [ ] #11: Stable device identity, persistent aliases, protected credentials, and explicit duplicate detection.
- [ ] #12: Independent per-camera states, hot arrival/removal, and bounded reconnection backoff.

Validation: simulated ONVIF responses and discovery failures; identity surviving IP changes; 16-camera state transitions with one failed camera; Windows/Linux network smoke checks. Never log credentials. Do not tie camera membership to selected previews.
