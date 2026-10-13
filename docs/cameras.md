# Camera discovery prototype

Select the local IPv4 address of the adapter attached to the camera network. On Windows,
use `Get-NetIPAddress -AddressFamily IPv4`; on Linux, use `ip -4 address`. Cameras need
UDP 3702 multicast and their HTTP/HTTPS ONVIF ports reachable through the firewall.

```sh
vidfish --discover --interface 192.168.1.10
vidfish --onvif http://192.168.1.20/onvif/device_service
```

Discovery sends a WS-Discovery probe on that adapter, collects correlated replies for
two seconds, and resolves up to 16 devices. Results include endpoint identity, device
information, H.264/H.265 media profiles, RTSP URIs, and optional snapshot URIs. Duplicate
identity/address pairs are deduplicated; conflicting addresses remain visible for
subsequent registry handling. Each camera failure appears alongside successful peers.
Commands do not start recordings or change settings. Repeat discovery to find arrivals.

If ONVIF is unavailable, use a known stream directly:

```sh
vidfish --preview rtsp://192.168.1.20/live
vidfish --record rtsp://192.168.1.20/live --output new-recording.mkv
```

The client supports ONVIF Media1, WS-Security UsernameToken PasswordDigest, and HTTP
Digest authentication. Continuous lifecycle behavior is described below. Media2-only H.265 devices may require manual RTSP fallback.
Camera URLs used by ONVIF must exclude embedded credentials. Request errors omit URLs,
server bodies, and credentials. HTTPS certificate validation remains enabled; redirects
and environment HTTP proxies are disabled for local camera requests. Each HTTP request
has a five-second timeout and a 1 MiB response limit. XML DTDs are rejected.

For repeatable tests, start `camera-fixture` from the media setup instructions and use
its printed device URL or the printed unicast discovery endpoint:

```sh
vidfish --discover --interface 127.0.0.1 --discovery-target 127.0.0.1:FIXTURE_PORT
```

The fixture port changes at startup. Production multicast must also be smoke tested on
the actual camera adapter; a loopback fixture cannot prove switch/firewall behavior.

## Persistent inventory

```sh
vidfish cameras scan --interface 192.168.1.10
vidfish cameras list
vidfish cameras add --alias "Bench left" --rtsp rtsp://192.168.1.20/live
vidfish cameras rename CAMERA_ID "Vehicle left"
vidfish cameras remove CAMERA_ID
```

Use `vidfish cameras --inventory PATH ...` for a separate inventory. The default is
`cameras.json` beside the platform settings file. Aliases belong to generated local IDs,
not IP addresses. An ONVIF endpoint with matching manufacturer/serial can move to a new
address without changing its alias. Reused UUIDs with different hardware, hardware with
a changed UUID, and duplicate UUIDs at multiple addresses are reported for operator
resolution. Unknown hardware cannot automatically authorize an address change. Inventory
writes are atomic; invalid files and failed validation preserve existing data. Up to 16
cameras can be stored. Removal changes inventory only; it does not stop a recording.

## Camera credentials

```sh
vidfish cameras add-onvif --alias "Bench right" --device http://192.168.1.21/onvif/device_service
vidfish cameras set-credentials CAMERA_ID --username-env CAMERA_USER --password-env CAMERA_PASSWORD
vidfish cameras resolve CAMERA_ID
vidfish cameras forget-credentials CAMERA_ID
```

Set the named environment variables using your OS's protected credential workflow;
never put secret values in command arguments or check them into files. Inventory stores
only a reference to the local camera ID. Windows credentials use Windows Credential
Manager; Linux uses the user's kernel keyring. Linux keyring entries persist across
process restarts but expire after the configured user-keyring lifetime and are lost
across reboot; re-enter credentials when needed. Restricted containers may deny keyring
access. Errors report this explicitly; there is no plaintext-file fallback.

Authentication sends a fresh nonce and timestamp with a password digest; a challenged
HTTP request retries once with Digest authorization. Camera clocks must agree with UTC.
The client refuses media service URLs on another host before sending credentials.
Passwords are cleared from owned secret buffers on drop. Device/network/vendor smoke
testing is still required; fixtures cannot establish compatibility with every camera.

## Continuous camera lifecycle

```sh
vidfish cameras watch --interface 192.168.1.10
vidfish cameras watch --seconds 30
```

Watch emits bounded JSON status updates until Ctrl+C or the optional time limit.
The desktop and `--service` start the same camera manager independently of previews.
Existing ONVIF inventory entries are polled even when discovery is disabled. To enable
continuous discovery in the desktop/service, enter the local adapter IPv4 in settings,
save, and restart. Custom `--config` files use `cameras.json` in the same directory.
A file lock permits one manager or inventory writer at a time. Stop the desktop/service/
watch before editing that inventory with CLI commands; concurrent read-only listing and
resolution remain available.

The manager creates at most 16 independent polling workers. Each profile-resolution
attempt has a 30-second overall budget; individual HTTP exchanges are limited to five
seconds. Discovery runs every ten seconds and collects replies for 500ms. Failed
connections retry after 1, 2, 4, 8… seconds, capped at 60 seconds. After three failures a
camera is shown offline but continues retrying. A failed poll clears obsolete profiles;
healthy peers continue. Identity conflicts pause the affected worker until the inventory
is explicitly corrected and the manager restarted. Shutdown joins workers after their
bounded in-flight requests finish.

`Available` means that ONVIF device/profile resolution succeeded. It does not prove RTSP
media flow or recording progress. Manual RTSP-only cameras are `ManualConfigured`, with
stream health established by the preview/recording operation. Actual recording state,
auto-record policy execution, and media-progress monitoring belong to Epic 3. The camera
manager never starts, stops, or owns a preview or recording pipeline. Renames are saved
on the coordinator worker, away from the UI thread.
