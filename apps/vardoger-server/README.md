# vardoger-server

Google sign-in, the account's devices, and the device-room relay between them.
Routes are listed at the top of `src/lib.rs`.

## Run

```sh
cargo build --release -p vardoger-server

VARDOGER_SERVER_URL=https://vardoger.example.com \
GOOGLE_CLIENT_ID=… \
GOOGLE_CLIENT_SECRET=… \
VARDOGER_ALLOWED_EMAILS=you@gmail.com \
VARDOGER_SERVER_DB=/var/lib/vardoger/server.db \
VARDOGER_SERVER_ADDR=127.0.0.1:8787 \
./target/release/vardoger-server
```

- `VARDOGER_ALLOWED_EMAILS`: comma-separated; only these Google accounts can sign in.
- `VARDOGER_SERVER_DB` defaults to `./vardoger-server.db`; `VARDOGER_SERVER_ADDR` to `127.0.0.1:8787`.

## Google OAuth client

Google Cloud Console → APIs & Services → Credentials → Create OAuth client ID →
Web application. Authorized redirect URI: `{VARDOGER_SERVER_URL}/auth/google/callback`.

## cloudflared

```yaml
ingress:
  - hostname: vardoger.example.com
    service: http://127.0.0.1:8787
  - service: http_status:404
```

WebSockets pass through a Cloudflare tunnel as is. Relay dials carry the
device token in the URL (`?token=`), which is how the engine's relay client
dials; keep request logging off on the tunnel and the server.
