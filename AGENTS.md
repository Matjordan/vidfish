# Development workflow

- Work on an epic branch, never directly on `main`.
- Commit each coherent change after its relevant checks pass; do not leave completed changes uncommitted.
- Maintain one PR per epic (#1–#6). Reuse its existing branch and PR as implementation progresses.
- Draft PRs may contain implementation plans; clearly distinguish plans from implemented acceptance criteria.
- Do not close an epic until its child issues and required platform checks are complete.
- Each push to `main`, including merged PRs, runs the versioned release workflow.
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

Epics #2–#6 currently contain planning documents only and branch from the initial
`main`. After the foundation merges, bring the updated `main` into each branch before
implementation. Preserve existing work and commit any conflict resolutions.
