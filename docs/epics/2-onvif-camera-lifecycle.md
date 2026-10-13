# Epic #2: ONVIF discovery and camera lifecycle

Implemented on the merged Rust foundation:

- #10: bounded WS-Discovery with selected IPv4 adapter, device/Media1 profile resolution,
  H.264/H.265 paths, and manual ONVIF/RTSP fallback through Add camera or CLI.
- #11: persistent local identities/aliases, hardware checks across address changes,
  duplicate/changed identity quarantine, and app-managed runtime login setup. The current
  closed deployment explicitly uses a local plaintext credential file; passwords are
  excluded from listings, arguments, logs, Git, and release packages.
- #12: independent camera polling, bounded reconnect backoff, hot arrival/removal,
  auto-record policy integration, per-camera recording state and file progress, and
  independent recording processes that survive UI/owner restarts.

See [camera workflows](../cameras.md). The desktop supports View feed, four bounded
preview slots, camera login, rename, manual addition, and recording controls. Basic
single-file recording is the policy bridge; segmentation and detailed recording/session
indexes belong to subsequent epics.

Validation covers real simulated SOAP/WS-Discovery traffic, authentication, stable aliases
after address changes, 16 polling workers and 16 acquisition entries with one failed peer,
hot arrival/disappearance/reconnection, runtime credentials, and actual owner-process death
with recording survival, reattachment without duplication, and finalized 1080p media.
Local checks pass. Native Windows/Linux review validation is the final checkpoint.
The operator reported the preceding camera inventory/login build works on the closed
system. Real switch/firewall and camera-vendor behavior should continue to be checked there.
