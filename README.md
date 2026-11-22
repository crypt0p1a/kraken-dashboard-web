# kraken-dashboard-web

> The web backend of [kraken-trading-bot](https://github.com/crypt0p1a/kraken-trading-bot): it
> keeps what the bot pushes at each tick, for its dashboard on the web

[![CI](https://github.com/crypt0p1a/kraken-dashboard-web/actions/workflows/ci.yml/badge.svg)](https://github.com/crypt0p1a/kraken-dashboard-web/actions/workflows/ci.yml)
[![📄 License: MIT](https://img.shields.io/badge/%F0%9F%93%84_license-MIT-blue)](LICENSE)

The bot keeps everything in its SQLite database, and its dashboard in the terminal reads it on the
machine of the bot. To follow the bot from elsewhere, the bot pushes, at the end of each tick,
every screen its dashboard can show to this backend, which keeps the last push in memory.

**For now, the backend only receives**: the web interface, a dashboard like the one of the
terminal, comes next. Read only: nothing goes back to the bot.

## Running it

```sh
docker compose up -d --build
docker compose logs -f
```

Then, in the `.env` of the bot: `KRAKEN_BOT_PUSH_URL=http://this-machine:8080`. The bot pushes as
soon as it starts, then at the end of each tick.

`KRAKEN_DASHBOARD_WEB_PUBLISH` is where the port is published on the machine, `127.0.0.1:8080` by
default: this machine only.

**Nothing is authenticated yet**: whoever reaches the port can replace what's shown. Beyond this
machine, only on a network of your own (a VPN such as WireGuard or Tailscale), never on the
internet.

Without Docker: `cargo run --release` (`--listen ADDRESS`, or `KRAKEN_DASHBOARD_WEB_LISTEN`,
`127.0.0.1:8080` by default).

## What the bot pushes

```text
kraken-trading-bot ── PUT /state (gzip, at each tick) ──▶ kraken-dashboard-web ◀── GET /data ── the web
```

| Route | From | What |
|-------|------|------|
| `PUT /state` | the bot | every screen: `{"at": 1795316768688, "views": [{"range": "day", "account": null, "data": {…}}]}`, compressed or not (`Content-Encoding: gzip`), the pushes taken one after the other in the order they come |
| `GET /version` | a reader | `{"version": …, "pushed_at": …}`: the version changes at each push, and from a run to the next |
| `GET /data?range=day&account=2` | a reader | the data of a screen as the bot pushed it, gzip to whoever accepts it; no `account`: all of them added up |

A screen is a range (`hour`, `day`, `week`, `month`, `all`) of all the accounts of the bot added
up, and of each account when there are several. The backend keeps its data as the bot wrote it,
never reading it: what it holds is the business of the bot and of the dashboard. Nothing is kept
on disk: started again, the backend has nothing until the next push of the bot.

A push of the demo database of the bot (8 managers, 3 pairs, 7 days) is about 720 KB compressed,
3.1 MB of JSON; a screen 14 KB (1 hour) to 290 KB (7 days) compressed.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

[MIT](LICENSE)
