# commutator-server

Google sign-in, the account's devices, and the device-room relay between them.
Routes are listed at the top of `src/lib.rs`.

## Run

```sh
cargo build --release -p commutator-server

COMMUTATOR_SERVER_URL=https://commutator.example.com \
GOOGLE_CLIENT_ID=… \
GOOGLE_CLIENT_SECRET=… \
COMMUTATOR_ALLOWED_EMAILS=you@gmail.com \
COMMUTATOR_SERVER_DB=/var/lib/commutator/server.db \
COMMUTATOR_SERVER_ADDR=127.0.0.1:8787 \
./target/release/commutator-server
```

- `COMMUTATOR_ALLOWED_EMAILS`: comma-separated; only these Google accounts can sign in.
- `COMMUTATOR_ICE_URLS` (optional): comma-separated `stun:`/`turn:`/`turns:` URLs
  handed to every device on its signaling socket, replacing the devices' default
  (Google's public STUN); `COMMUTATOR_TURN_USERNAME` and `COMMUTATOR_TURN_CREDENTIAL`
  sign in to the TURN ones. Without TURN, two devices that are both behind
  symmetric NAT (many cellular networks) can't connect directly: terminals stay on
  the relay, previews don't open.
- `COMMUTATOR_SERVER_DB` defaults to `./commutator-server.db`; `COMMUTATOR_SERVER_ADDR` to `127.0.0.1:8787`.

## Google OAuth client

Google Cloud Console → APIs & Services → Credentials → Create OAuth client ID →
Web application. Authorized redirect URI: `{COMMUTATOR_SERVER_URL}/auth/google/callback`.

## cloudflared

```yaml
ingress:
  - hostname: commutator.example.com
    service: http://127.0.0.1:8787
  - service: http_status:404
```

WebSockets pass through a Cloudflare tunnel as is. Relay dials carry the
device token in the URL (`?token=`), which is how the engine's relay client
dials; keep request logging off on the tunnel and the server. Preview
signaling (`/preview/ws`) is a WebSocket too; the WebRTC traffic it sets up
goes device to device (or through TURN), never through this server.
