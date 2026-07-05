# SettleUp

A small web app for splitting shared expenses — a bar tab among strangers or a
running monthly split between two people. One person creates a group, everyone else
joins by scanning a QR / opening a link (no accounts, no install), each adds what they
paid, and the app computes the fewest payments that settle everyone up.

Built with Rust (`axum` + `maud` + `htmx`) and SQLite. See
[`docs/DECISIONS.md`](docs/DECISIONS.md) for the design rationale.

## Run locally

```sh
cargo run
# -> SettleUp listening on http://127.0.0.1:3000
```

Open <http://127.0.0.1:3000>, create a group, and share the link it gives you.

## Configuration

All configuration is via environment variables:

| Variable              | Default            | Purpose                                                        |
| --------------------- | ------------------ | -------------------------------------------------------------- |
| `SETTLEUP_ADDR`       | `127.0.0.1:3000`   | Address to bind.                                               |
| `SETTLEUP_DB`         | `settleup.db`      | SQLite file path (created if missing).                         |
| `SETTLEUP_BASE_URL`   | *(derived)*        | Public base URL used to build join/QR links. **Set in prod.** |
| `SETTLEUP_SECURE`     | *(auto)*           | `1`/`true` to force `Secure` cookies. Auto-on when base URL is `https`. |

## Deploying

The server speaks plain HTTP; run it behind a TLS-terminating reverse proxy
(Caddy, nginx, …). Two things **must** be set for a correct deployment:

1. **`SETTLEUP_BASE_URL=https://your.host`** — otherwise the QR code and invite
   link are built from the request's `Host` header as `http://…`, which breaks when a
   phone scans it against your HTTPS site (mixed content / wrong scheme).
2. **HTTPS** so the `Secure` cookie (auto-enabled by the `https` base URL) is sent.
   Cookies are `HttpOnly` + `SameSite=Lax`; identity is a per-device token, so losing
   the cookie means losing access to that group unless a recovery passphrase was set.

Example:

```sh
SETTLEUP_ADDR=0.0.0.0:3000 \
SETTLEUP_BASE_URL=https://settleup.example \
SETTLEUP_DB=/var/lib/settleup/settleup.db \
  ./settleup
```

Groups with no recovery passphrase are auto-deleted after a few days of inactivity
(so throwaway bar tabs clean themselves up); set a recovery passphrase to keep one.

## Tests

```sh
cargo test
```

Covers the money parsing, balance math, debt simplification, and the auto-expiry
cascade.

## License

Licensed under the GNU General Public License v3.0 (`GPL-3.0-only`). See
[`LICENSE`](LICENSE).

## Known v1 limitations

- No in-place expense editing (delete + re-add).
- No CSRF tokens (mitigated by `SameSite=Lax` cookies); add tokens if hardening.
- No Content-Security-Policy yet: the app uses inline `<style>`/`<script>`, so a strict
  CSP needs `'unsafe-inline'` or nonces first. (htmx is now vendored and self-served —
  `assets/htmx-2.0.4.min.js`, embedded in the binary — so there's no third-party script
  origin to allow; it remains a progressive enhancement and the app works without it.)
- Single currency per group; no multi-currency, itemized splits, or notifications.
