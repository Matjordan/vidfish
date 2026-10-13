# Epic #2: ONVIF discovery and camera lifecycle

Builds on the merged Rust foundation in epic #1. Implementation is in progress.

Implemented: bounded WS-Discovery probes with explicit IPv4 adapter selection,
correlated response collection, ONVIF device/Media1 profile resolution, and CLI
manual ONVIF/RTSP fallback. See [camera instructions](../cameras.md). Runtime login setup and local credential storage, WS-Security/HTTP Digest authentication, atomic camera inventory,
persistent aliases, identity conflict quarantine, and independent camera polling workers
are implemented. The desktop lists camera state/errors and supports persistent renames;
`--service` and `cameras watch` run discovery without a desktop. Tests cover 16 workers
with one failure and hot arrival/disappearance/reconnection.

- [ ] #10: WS-Discovery with interface selection, ONVIF profiles, RTSP URI resolution, and manual fallback.
- [ ] #11: Stable device identity, persistent aliases, app-managed credentials for the current closed deployment, and explicit duplicate detection.
- [ ] #12: Independent per-camera states, hot arrival/removal, and bounded reconnection backoff.

Validation: simulated ONVIF responses and discovery failures; identity surviving IP changes; 16-camera state transitions with one failed camera; Windows/Linux network smoke checks. Never log credentials. Do not tie camera membership to selected previews.

Remaining: actual recording-policy execution and recording-state feedback require the
acquisition implementation in Epic 3. Manual RTSP health is established by media workers,
not ONVIF polling. Native Windows validation is deferred until ready for review; real
multicast/vendor-camera smoke testing remains necessary. Do not mark the epic complete
based solely on loopback fixtures.
