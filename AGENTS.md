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
