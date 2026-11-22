# kraken-dashboard-web

> The web backend of [kraken-trading-bot](https://github.com/crypt0p1a/kraken-trading-bot): it
> keeps what the bot pushes at each tick, for its dashboard on the web

[![CI](https://github.com/crypt0p1a/kraken-dashboard-web/actions/workflows/ci.yml/badge.svg)](https://github.com/crypt0p1a/kraken-dashboard-web/actions/workflows/ci.yml)
[![📄 License: MIT](https://img.shields.io/badge/%F0%9F%93%84_license-MIT-blue)](LICENSE)

The bot keeps everything in its SQLite database, and its dashboard in the terminal reads it on the
machine of the bot. To follow the bot from elsewhere, the bot pushes, at the end of each tick,
every screen its dashboard can show to this backend, which keeps the last push in memory and
serves **the same dashboard in a web page**: http://127.0.0.1:8080/.

The page runs the dashboard of the terminal itself, its screen and its logic compiled to
WebAssembly and drawn by [ratzilla](https://github.com/ratatui/ratzilla) (ratatui in the browser):
the same panels, the same charts, the same keys (`d`, `1`…`5`, `a`, `⏎`, `tab`, the arrows, `?`).
Read only: nothing goes back to the bot, a tick is asked for from the dashboard of the terminal.

## Running it

```sh
cp .env.example .env    # KRAKEN_DASHBOARD_WEB_PUSH_KEY: openssl rand -hex 32
docker compose build
docker compose run --rm -it kraken-dashboard-web hash-password   # its line in .env
docker compose up -d
docker compose logs -f
```

Then, in the `.env` of the bot: `KRAKEN_BOT_PUSH_URL=http://this-machine:8080`, and
`KRAKEN_BOT_PUSH_KEY`, the same key. The bot pushes as soon as it starts, then at the end of each
tick.

`KRAKEN_DASHBOARD_WEB_PUBLISH` is where the port is published on the machine, `127.0.0.1:8080` by
default: this machine only.

**The pushes are signed**: HMAC-SHA256 with a key only the bot and the backend know, never sent.

```text
X-Kraken-Push-At: 1795316768688              when the bot signed, UTC milliseconds
X-Kraken-Push-Signature: sha256=<hex>        HMAC-SHA256(key, "<at>\n" + the body as sent)
```

A push not signed with the key is refused before it's read (401), one signed more than 5 minutes
away from the clock of the backend too, and one signed at or before the last one taken (409): a
push heard on the way can't be sent again, nor one held back to replace a newer one.

**The dashboard wants a password**: the page and what it reads, with a session only. The password
is kept hashed (argon2id) in `KRAKEN_DASHBOARD_WEB_PASSWORD_HASH`, made by `hash-password`; logged
in, a session of a month in a cookie (`HttpOnly`, `SameSite=Strict`, `Secure`), `/logout` to end it.
Five wrong passwords in a minute and the next are refused until the minute is over. Served in
plain HTTP, `KRAKEN_DASHBOARD_WEB_COOKIE_SECURE=false`; on the internet, behind HTTPS always (nginx,
Caddy).

Without Docker, the page first, then the backend serving it, `KRAKEN_DASHBOARD_WEB_PUSH_KEY` in
its environment:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked trunk --version "~0.21"
(cd front && trunk build --release)   # front/dist
cargo run --release                   # --listen ADDRESS, KRAKEN_DASHBOARD_WEB_LISTEN, 127.0.0.1:8080 by default
```

The page is served from `KRAKEN_DASHBOARD_WEB_PAGE`, else `front/dist`; without it, the routes only.

## What the bot pushes

```text
kraken-trading-bot ── PUT /state (gzip, at each tick) ──▶ kraken-dashboard-web ◀── GET /data ── the web
```

| Route | From | What |
|-------|------|------|
| `PUT /state` | the bot | every screen: `{"at": 1795316768688, "views": [{"range": "day", "account": null, "data": {…}}]}`, compressed or not (`Content-Encoding: gzip`), signed, the pushes taken one after the other in the order they come |
| `GET /version` | a reader | `{"version": …, "pushed_at": …}`: the version changes at each push, and from a run to the next |
| `GET /data?range=day&account=2` | a reader | the data of a screen as the bot pushed it, gzip to whoever accepts it; no `account`: all of them added up |
| `GET /` | a browser | the page, compressed: the dashboard reading the routes above, a screen at a time, when the bot pushed or a key changes what's shown |

A screen is a range (`hour`, `day`, `week`, `month`, `all`) of all the accounts of the bot added
up, and of each account when there are several. The backend keeps its data as the bot wrote it,
never reading it: what it holds is the business of the bot and of the dashboard. Nothing is kept
on disk: started again, the backend has nothing until the next push of the bot.

A push of the demo database of the bot (8 managers, 3 pairs, 7 days) is about 720 KB compressed,
3.1 MB of JSON; a screen 14 KB (1 hour) to 290 KB (7 days) compressed.

## Development

```text
src/     the backend (axum)
front/   the page: the dashboard of kraken-trading-bot (its crate kraken-dashboard, without the
         terminal, at a commit of crypt0p1a/kraken-trading-bot#12 until it's merged) in
         WebAssembly, built by trunk
```

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy -p kraken-dashboard-web-front --target wasm32-unknown-unknown -- -D warnings
```

## License

[MIT](LICENSE)
