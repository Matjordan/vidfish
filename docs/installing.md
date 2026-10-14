# Install, update and uninstall Vidfish

Release downloads are at <https://github.com/Matjordan/vidfish/releases>.
All filenames, the release title/tag and `vidfish --version` use the same version.
Portable archives remain available alongside installers and updater payloads.

## Supported systems and dependencies

Windows 10/11 x86_64 (including an x86_64 Toughpad): use
`vidfish-VERSION-windows-x86_64-setup.exe`. Installation defaults to
`%LOCALAPPDATA%\Programs\Vidfish`, creates a Start Menu entry and Windows uninstall
entry, and needs no administrator access. The complete GStreamer runtime, plugins,
scanner, presets/notices and Microsoft app-local C++ runtime DLLs are included.
Microsoft DLL/redistributable signatures are validated while packaging. Vidfish
installers/binaries are currently **unsigned**; those Microsoft signatures do not
sign Vidfish. Install only releases you trust. No certificate is currently configured. Maintainers can configure GitHub Actions secrets
`VIDFISH_SIGN_CERT_BASE64` (PFX certificate) and `VIDFISH_SIGN_CERT_PASSWORD`;
the release job then signs both executables before payload hashing and the installer
after compilation, with an authenticated timestamp. A signing failure fails packaging.

Linux x86_64: Ubuntu 22.04+ or a compatible glibc distribution with GStreamer 1.20+
and a working OpenGL/X11/Wayland desktop. The Linux release deliberately uses the
system media/graphics runtime; it does not bundle a distribution's shared libraries.
On Ubuntu/Debian, an administrator provisions dependencies once:

```sh
sudo apt-get install libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  libxkbcommon0 libxkbcommon-x11-0 libgl1
```

The installer and updater never run sudo or request elevation. Missing loader
libraries or H.264/H.265/RTSP plugins cause a failed readiness check before installing
or accepting an update. See [media.md](media.md) for plugin diagnostics.

## Linux user-local installation

Download the Linux tarball and `SHA256SUMS` from the same release. In a directory
containing the downloaded archive:

```sh
sha256sum --ignore-missing --check SHA256SUMS
tar -xzf vidfish-VERSION-linux-x86_64.tar.gz
cd vidfish-VERSION
./install.sh
~/.local/bin/vidfish --version
```

Verify that the archive you downloaded was checked successfully. Defaults are
`~/.local/bin/vidfish`, `~/.local/share/applications/vidfish.desktop` and hicolor icons.
`./install.sh --prefix /absolute/writable/prefix` supports a custom user-writable
prefix (including spaces). Custom prefixes need their `bin` on PATH and their
`share` on XDG_DATA_DIRS for desktop menu/icon discovery; running the installed
launcher directly also works. Files with modified contents or unowned launchers
are never overwritten. Incorrect architecture or unavailable dependencies are errors.
The package also runs portably using `./launch-vidfish.sh` without installation.

## Data directories and ownership

Only application code/runtime belongs in `PREFIX/lib/vidfish/versions/VERSION`.
The active version is selected by `current.txt`. `install.json` tracks the exact
integration files with their SHA256 values; each version's `bundle.json` tracks
its complete immutable contents. Older bundles remain available for rollback.

Operator config, camera inventory, aliases/views, plaintext credentials and
acquisition status live beside the settings file: Linux normally
`~/.config/vidfish/config.json`, Windows normally
`%APPDATA%\vidfish\vidfish\config.json`. `--config /path/config.json` overrides it.
Recordings, session/event manifests, indexes, snapshots and exports live under the
configured output directory. The default is `recordings` relative to the launch
working directory; set an absolute output directory in Settings for deployment.
Never place scientific data in an application bundle. Install/uninstall does not
migrate or remove the operator data directories; an updated app reopens its normal
settings and session history. No schema migration is
introduced by this epic; unsupported existing config schemas still report an error.

## Safe in-app updates

Startup checks official stable GitHub releases on a worker with a 15-second timeout.
Offline, current, missing-platform and prerelease results do not interrupt the UI
or show an automatic prompt. A newer compatible release displays version, release
notes and download size. Update Now starts a download only after operator choice;
Later dismisses that prompt for the session. Settings → Check for updates also
works when automatic checks are disabled. Save Settings to persist the automatic
check choice. `vidfish --no-update-check` suppresses startup checks on offline rigs.

While recording, the choice becomes **Download and defer until safe**. Recording
continues; the updater never sends Stop All, changes auto-record policy or terminates
a recorder. Finish acquisition and stop recordings explicitly when ready. The
pending install can be canceled while downloading or waiting; a completed verified
payload can later be installed from the update window. Downloads stay in staging,
never in the current bundle. Failed/partial downloads are discarded automatically.

The payload is checked against the release's `SHA256SUMS`, then every bundle file is
checked against its manifest. URLs and filenames must match the official repository
and current OS/architecture; unsafe archive paths, symlinks, duplicates, excess sizes
and mismatched versions are rejected. SHA256 detects corruption against that release;
it **is not a separately authenticated publisher signature**.

Each installed recorder holds a shared `recording.lock` through pipeline EOS and
metadata persistence. Maintenance must obtain its exclusive side before replacement,
including after the desktop exits; a new recorder cannot start while it is held.
The separate `process.lock` keeps every desktop, CLI and service attached to the
installed bundle until exit. Other open desktops/services cause the helper to time
out without replacing files. Stop those processes yourself when recording is finished.
There is no implied stop command. Unmanaged older portable processes cannot join
this install lock protocol: close them explicitly before installing or updating.

The helper stages a full version (including Windows media runtime), writes a synced
transaction journal, atomically switches the version pointer, then checks reported
version, isolated configuration startup and H.264/H.265/RTSP media prerequisites.
A desktop update then relaunches Vidfish with the same config path in probation:
recording stays gated until the actual UI renders and confirms the expected version.
Automatic recording settings are not changed; their normal behavior resumes only
after confirmation. Failed startup closes that probationary UI before recording can
start, restores the previous pointer and relaunches the old version. Successful checks
create `health.json`; failure records a recovery message. These bounded checks do not
certify a physical GPU/camera or prove
that a later operator session cannot fail; check the launched desktop on your rig.

An interrupted transaction is recovered by the installed launcher before starting
Vidfish. Recovery returns to the previous verified bundle. Read-only/system-managed
or portable installs use the release installer/package manager manually, without
an elevation request from Vidfish. Free space and writable install/staging directories
are required. Recovery details are in `PREFIX/lib/vidfish/update-error.txt` and
`update-helper.log`; the next launch surfaces the error in the update window.

## Manual update and rollback

Close all Vidfish desktops/services after explicitly finishing recordings. Verify
checksums for the new release, then run its Windows installer or Linux `install.sh`
with the original prefix. Settings/credentials/output are retained.

To roll back, install the trusted old release again using the same prefix, or invoke
an already retained bundle's maintenance helper while all Vidfish processes are closed:

```sh
~/.local/lib/vidfish/versions/OLD/vidfish-maintenance install \
  --prefix "$HOME/.local" --bundle "$HOME/.local/lib/vidfish/versions/OLD"
```

Windows uses `vidfish-maintenance.exe` with the same arguments and the Windows prefix.
This is an explicit rollback, unlike the stable automatic updater, which offers newer
versions only. If recovery needs to be invoked manually, use `recover --root
PREFIX/lib/vidfish`. Do not delete the transaction or alter pointers while recording.

## Uninstall

Linux: run the installed or downloaded release's `uninstall.sh` (same `--prefix` used
at installation), or its `vidfish-maintenance uninstall --prefix PREFIX`. Windows:
use Settings → Installed apps → Vidfish. Close desktops/services and finish recordings
first; active process/recording locks refuse removal.

Only manifest-owned unchanged files are removed. Unknown/modified files and all
settings, credentials and scientific output are retained. Reinstall with the same
prefix to reuse data. Empty control/lock directories may remain. Staged downloads
are retained separately for diagnosis/retry and can be removed manually when no
update/helper is running. Deleting scientific output is a separate operator action.

## Verification and deployment status

Linux CI runs real previous-version install/upgrade, reinstall/uninstall, version and
runtime readiness, corrupt/wrong-architecture rejection, failed-health rollback,
interrupted-journal recovery and actual RTSP recording protection. Rust tests cover
release selection, checksum parsing, archive traversal and file ownership. The native update smoke also checks an offered release and Later with four growing
recorders, manual checks with auto disabled, and real version-confirmed desktop
restart/failure rollback. Existing native GUI/media/acquisition smoke checks continue
to run. An explicit `VIDFISH_UPDATE_PROBE_RELEASE` metadata fixture is accepted only
when `VIDFISH_UI_PROBE` is enabled, and still uses the strict official-URL parser; it
does not supply an alternative download endpoint.

Windows release compilation/packaging and package readiness remain enabled.
Windows installer/update and native UI tests are implemented behind `windows_tests`
but remain **paused at the operator's request**. A passed Linux test is not a Windows
result. A physical Toughpad installation remains to be verified before deployment:
install as a standard user, check Start Menu/version/runtime, record four streams,
offer/defer an update and confirm streams keep progressing, explicitly stop them,
complete the update, test rollback and uninstall, then verify retained data. Do not
claim these physical checks were performed by the cloud CI.
