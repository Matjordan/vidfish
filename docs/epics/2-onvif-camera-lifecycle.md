# Epic #2: ONVIF discovery and camera lifecycle

Depends on the Rust foundation in epic #1. Implementation has not started.

- [ ] #10: WS-Discovery with interface selection, ONVIF profiles, RTSP URI resolution, and manual fallback.
- [ ] #11: Stable device identity, persistent aliases, protected credentials, and explicit duplicate detection.
- [ ] #12: Independent per-camera states, hot arrival/removal, and bounded reconnection backoff.

Validation: simulated ONVIF responses and discovery failures; identity surviving IP changes; 16-camera state transitions with one failed camera; Windows/Linux network smoke checks. Never log credentials. Do not tie camera membership to selected previews.
