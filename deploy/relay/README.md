# notdot relay behind a reverse proxy (proof of concept)

Status: **local proof of concept only. Nothing here is deployed.**

These files run the official `iroh-relay` (v1.4.1 container) on a private container network, behind an nginx reverse proxy that terminates TLS and forwards the relay's HTTP Upgrade connection. This is the shape proposed for a host where another proxy already owns ports 80/443: the relay gets its own hostname on the existing proxy and opens no new public ports.

The relay only forwards packets that are already end-to-end encrypted between the desktop and the phone (QUIC/TLS between their keys). It can see which keys talk, when, and how much.

## Run locally

```bash
mkdir -p .local/relay-certs
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 7 -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" -keyout .local/relay-certs/relay.key -out .local/relay-certs/relay.crt
chmod 644 .local/relay-certs/relay.key
(cd deploy/relay && RELAY_CERT_DIR=../../.local/relay-certs docker compose up -d)
NOTDOT_RELAY_URL=https://localhost:8443 NOTDOT_RELAY_CA=.local/relay-certs/relay.crt cargo test -p notdot-link --features test-relay --test reverse_proxy -- --ignored --test-threads=1
(cd deploy/relay && RELAY_CERT_DIR=../../.local/relay-certs docker compose down)
```

Ports bind to 127.0.0.1 only. The tests verify the relay certificate against the generated root; no verification is skipped. `NOTDOT_PROXY_OUTAGE_SECS` sets the outage used by the reconnect test (default 10).

## What the tests show (local loopback, synthetic data)

| Scenario | Result |
| --- | --- |
| Pairing and 20 relay-only calls through the proxy (port 8443) | Pass |
| Same proxy without `Upgrade`/`Connection` forwarding (port 8444) | Relay never comes online; relay-only connection fails, as expected |
| Proxy restarted, or down for 10 s, during a call | The connection survives; the call completes; a retry returns the recorded outcome (one commit) |
| Proxy down for 45 s (longer than the QUIC idle timeout) | The connection is lost; the phone reconnects on the first attempt; the retry returns the recorded outcome (one commit) |

Plain HTTP health checks pass on both ports, so a health check alone cannot detect a missing Upgrade configuration. Monitor a real relay connection instead.

These results say nothing about WAN latency, mobile networks or load.

## Proposed deployment (needs owner approval before any change)

- One container from `compose.yaml`: read-only root filesystem, unprivileged user, all capabilities dropped, 1 CPU, 256 MiB memory, 256 processes, log rotation, a private network with no links to other services.
- On the existing reverse proxy: one server block for a new relay hostname, using the `location /` block from `nginx.conf` with a certificate from the proxy's existing ACME setup. No new public ports. QUIC address discovery (UDP) stays off unless a UDP port is approved separately.
- Access control: shared token or HTTP allowlist callout (iroh-relay `access`), never an open relay. Per-client rate limit as in `relay.toml`.
- Budgets (targets): CPU at most 1 core, memory at most 256 MiB, disk at most 100 MB. Alert at 50 GB/month of relay egress; review at 200 GB/month.
- Rollback: `docker compose down`; remove the proxy server block and the DNS record. No other service, file or setting changes, so rollback is complete. Phones then fall back to "computer unreachable" away from home; on the same network they still connect directly.
- Production needs at least two relays in different regions (iroh guidance). One relay on a shared host suits a private beta only.
