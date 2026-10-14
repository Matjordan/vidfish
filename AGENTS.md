# Development workflow

- Work on an epic branch, never directly on `main`.
- Commit each coherent change after its relevant checks pass; do not leave completed changes uncommitted.
- Maintain one PR per epic. Reuse its existing branch and PR as implementation progresses.
- Draft PRs may contain implementation plans; clearly distinguish plans from implemented acceptance criteria.
- Do not close an epic until its child issues and required platform checks are complete.
- Windows checks are paused at the user's request, including ready PRs and manual runs. Run Linux CI only; do not resume Windows testing without user direction. Releases still compile and package Windows, with only the package version/launch sanity check; Windows test suites and GUI smoke are skipped. The reusable workflow has a `windows_tests` switch for future restoration.
- Each push to `main`, including merged PRs, runs the versioned release workflow.
- For the current closed deployment, credentials use an app-managed local plaintext file and runtime login dialog. Do not require manual OS keyring setup. Keep passwords out of logs. If secure storage is added later, provision it during install/runtime.
- Keep recording independent of preview, UI, and event operations. Support Windows and Linux.
- Activate the cloud Rust toolchain with `CARGO_HOME=/workspace/.cargo`, `RUSTUP_HOME=/workspace/.rust`, and `/workspace/.cargo/bin` on PATH when needed.
- Run formatting, relevant tests, and Clippy before committing Rust changes. Use the committed Cargo lockfile.

## Epic branches and PRs

| Epic | Branch | PR |
| --- | --- | --- |
| #1 Foundation | `epic/1-rust-foundation` | https://github.com/Matjordan/vidfish/pull/26 |
| #2 Cameras | `epic/2-onvif-camera-lifecycle` | https://github.com/Matjordan/vidfish/pull/27 |
| #3 Recording | `epic/3-continuous-recording` | https://github.com/Matjordan/vidfish/pull/28 |
| #4 Monitoring | `epic/4-live-monitoring` | https://github.com/Matjordan/vidfish/pull/29 |
| #5 Events | `epic/5-scientific-events` | https://github.com/Matjordan/vidfish/pull/30 |
| #6 Sessions | `epic/6-session-management` | https://github.com/Matjordan/vidfish/pull/31 |

Epic #2 contains implemented discovery, identity/credential storage, and camera polling
plus a hot-arrival recording-policy bridge and in-app camera previews on top of the merged foundation. Epic #3 adds segmented recording, disk policy, and durable recovery indexes.
Epic #4 adds the touch operator layout, persistent selections, fullscreen, resource
bounds, and a selected-preview event bridge. The user deferred numerical Toughpad
CPU/GPU measurements and reported that four streams looked fine; do not block on or
invent those numbers.
Epic #5 implements scientific markers, actual-boundary recording references,
bounded all-camera captures, editable notes and durable JSON/CSV recovery.
Epic #6 implements durable acquisition sessions, filtered media/event browsing,
portable event bundles, storage estimates and opt-in audited retention. Windows
checks remain paused as specified above.

Follow-ups #32–#34 and the Epic 6 release recovery-smoke fix share PR #35 on
`fix/session-recovery-smoke`, explicitly requested by the user. They implement
operator polish, automatic active-IPv4 discovery and shared login defaults.
Authentication rejection pauses ONVIF retries until credentials change or the manager
restarts. Credential values remain outside AppConfig/session metadata. Preserve
this behavior and the continued Windows-check pause.

Issue #36 uses `feat/36-camera-list-cleanup` for persistent roster hiding/restoration,
confirmed offline hiding, and recorder-lock-guarded inventory forgetting. Never remove
scientific files during camera cleanup or turn hiding into a discovery/recording policy.

Epic #41 (#37–#40) uses `epic/41-installers-updater` for per-user installers,
owned immutable version bundles, official stable-release update prompts and
recording-safe maintenance. Preserve the exclusive recording gate across runtime
checks and desktop startup confirmation; do not send Stop All or mutate operator
settings during update. Install/uninstall ownership manifests must never claim
scientific files or follow symlinks. Windows installer/native update smoke checks
are implemented under `windows_tests`. The user explicitly authorized Windows
checks for PR #43 (`epic/41-installers-updater`); run and fix its full Windows
suite. The general pause still applies to other branches. Physical Toughpad installer
verification is pending; do not close this epic until its required platform checks
are actually complete or the user explicitly changes those acceptance criteria.
