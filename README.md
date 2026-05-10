<p align="center">
  <img src="logo.png" alt="Teleemix Logo" width="200"/>
</p>

# Teleemix

A fast, lightweight Telegram bot written in Rust for requesting music downloads via your self-hosted [deemix](https://github.com/bambanah/deemix) instance — from your phone, without ever touching the deemix web UI or dealing with ARL logins.

Supports Deezer URLs, Spotify links, and free-text search.

---

## Features

- 🎵 Send a **Deezer URL** (track, album, playlist) → queued instantly
- 🟢 Send a **Spotify link** (track or album) → looked up and queued automatically
- 🔍 Send a **song or artist name** → search results shown as buttons to pick from
- 📲 `/menu` — quick action keyboard buttons
- 💿 `/album` — search albums
- 🔔 `/register` — get notified when the bot restarts
- 🔄 `/updatearl` — update your Deezer ARL interactively via Telegram
- 📊 `/status` — shows pending, downloading, and completed queue items separately
- 🔒 Optional user allowlist to restrict access
- ⚡ Written in Rust — tiny memory footprint, static binary, no runtime dependencies

---

## Requirements

- A running [deemix](https://github.com/bambanah/deemix) instance
- Docker + Docker Compose
- A Telegram bot token (from [@BotFather](https://t.me/BotFather))
- A Deezer account (free or paid; paid required for lossless)

---

## Setup

### 1. Create a Telegram bot

1. Open Telegram and message [@BotFather](https://t.me/BotFather)
2. Send `/newbot` and follow the prompts
3. Copy the token it gives you

### 2. Get your Deezer ARL

For instructions on how to obtain your Deezer ARL token, refer to the [deemix documentation](https://github.com/bambanah/deemix#arl).

The ARL lasts several months. When it expires, use `/updatearl` in Telegram to update it without touching the server.

### 3. Configure

```bash
cp .env.example .env
nano .env
```

Fill in all the values — see `.env.example` for descriptions of each variable.

### 4. Configure volume mounts

Edit `docker-compose.yml` and update the left side of these volume mounts to match your setup:

```yaml
volumes:
  - /path/to/your/docker-compose.yml:/compose/docker-compose.yml
  - /path/to/your/.env:/app/.env
  - /path/to/your/data/registered_users.txt:/app/registered_users.txt
  - /var/run/docker.sock:/var/run/docker.sock
```

Create the registered users file before starting:
```bash
touch /path/to/your/data/registered_users.txt
```

### 5. Deploy

```bash
docker compose pull
docker compose up -d
```

---

## Usage

| Action | How |
|---|---|
| Download a track | Send a Deezer or Spotify link, or just type the song name |
| Search tracks | `/search` or tap 🔍 Search a track in /menu |
| Search albums | `/album` or tap 💿 Search an album in /menu |
| Download a Deezer URL | `/dl` |
| Download from Spotify | `/sp` |
| Check deemix status | `/status` |
| Register for notifications | `/register` |
| Update ARL | `/updatearl` |
| Show all buttons | `/menu` |

---

## Updating

### Manual update

```bash
docker compose pull
docker compose up -d
```

### Automatic updates with Watchtower

If you want Teleemix to update itself automatically whenever a new image is published, you can use [Watchtower](https://github.com/containrrr/watchtower) or an alternative like [Diun](https://github.com/crazy-max/diun).

Watchtower example — add to your compose file:

```yaml
  watchtower:
    image: containrrr/watchtower
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
    command: --interval 86400 teleemix
    restart: unless-stopped
```

This checks for a new `teleemix` image once a day and updates automatically.

> ⚠️ Only recommended if you trust the source. For production use, pin to a specific image digest instead.

---

## Access Control

Set `ALLOWED_USERS` in your `.env` to a comma-separated list of Telegram user IDs:

```
ALLOWED_USERS=123456789,987654321
```

Leave empty to allow anyone who messages the bot. Fine for private bots, not recommended if the bot token is shared publicly.

To find your Telegram user ID, message [@userinfobot](https://t.me/userinfobot).

---

## Notifications on restart

Send `/register` to the bot once. From then on, every time the container restarts you will receive a message letting you know the bot is back online.

---

## Updating the ARL

When your Deezer ARL expires, send `/updatearl` to the bot. It will prompt you to send the new ARL, then:
1. Re-logs into deemix immediately
2. Updates the ARL in your `.env` file so it persists across restarts

---

## Docker tags

| Tag | Branch | Description |
|---|---|---|
| `latest` | `main` | Stable release |
| `dev` | `dev` | Experimental / in development |

To run the experimental version, change the image tag in `docker-compose.yml`:

```yaml
image: ghcr.io/asupersheep/teleemix:dev
```

---

## Built with

- [Rust](https://www.rust-lang.org/)
- [teloxide](https://github.com/teloxide/teloxide) — Telegram bot framework
- [tokio](https://tokio.rs/) — async runtime
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP client

---

## License

MIT
