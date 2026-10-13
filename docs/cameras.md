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

This first implementation supports unauthenticated ONVIF Media1 services. Authenticated
ONVIF, persistent manual cameras/aliases, continuous polling, and automatic reconnection
are subsequent parts of epic #2. Media2-only H.265 devices may require manual RTSP fallback.
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
