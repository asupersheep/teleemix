# Teleemix

A fast, lightweight Telegram bot written in Rust for requesting music downloads via your self-hosted [deemix](https://github.com/bambanah/deemix) instance — from your phone, without ever touching the deemix web UI or dealing with ARL logins.

Supports Deezer URLs, Spotify links, and free-text search.

---

## Features

- 🎵 Send a **Deezer URL** (track, album, playlist) → queued instantly
- 🟢 Send a **Spotify link** → title and artist resolved automatically, then matched on Deezer
- 🔍 Send a **song or artist name** → search results shown as buttons to pick from
- 💿 `/album` search
- 🔄 `/updatearl` — update your Deezer ARL via Telegram (updates compose file and rebuilds automatically)
- 🔒 Optional user allowlist to restrict access to yourself
- ⚡ Written in Rust — tiny memory footprint, fast startup, single binary

No Spotify API key required — uses the Spotify embed page, completely free.

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

1. Log in to [deezer.com](https://deezer.com) in your browser
2. Open DevTools (F12) → Network tab → refresh the page
3. Click any request to `deezer.com` → Request Headers → find the `Cookie` header
4. Copy the value between `arl=` and the next `;`

The ARL is ~190 characters and lasts several months. When it expires, use `/updatearl <new_arl>` in Telegram to update it without touching the server.

### 3. Configure

```bash
cp .env.example .env
nano .env
```

Fill in all the values — see `.env.example` for descriptions of each variable.

### 4. Configure volume mounts

Edit `docker-compose.yml` and update the left side of this volume to point to your actual compose file on the host:

```yaml
volumes:
  - /path/to/your/docker-compose.yml:/compose/docker-compose.yml
```

This allows the `/updatearl` command to persist ARL changes across container restarts.

### 5. Deploy

```bash
docker compose pull
docker compose up -d
```

---

## Updating

To update to the latest version:

```bash
docker compose pull
docker compose up -d
```

---

## Usage

| Action | How |
|---|---|
| Download a track | Send a Deezer or Spotify link, or just type the song name |
| Search tracks | `/search <song name>` |
| Search albums | `/album <album name>` |
| Download a Deezer URL | `/dl <url>` |
| Check deemix status | `/status` |
| Update ARL | `/updatearl <new_arl>` |

---

## Access Control

Set `ALLOWED_USERS` in your `.env` to a comma-separated list of Telegram user IDs:

```
ALLOWED_USERS=123456789,987654321
```

Leave empty to allow anyone who messages the bot. Fine for private bots, not recommended if the bot token is public.

To find your Telegram user ID, message [@userinfobot](https://t.me/userinfobot).

---

## How Spotify links work

Teleemix does **not** use the Spotify Web API and requires **no Spotify credentials**. Instead it:

1. Scrapes the Spotify embed page to extract the track title and artist
2. Searches deemix with that info
3. Shows you the top results as buttons to confirm

No setup needed — it works out of the box.

---

## Updating the ARL

When your Deezer ARL expires (every few months), send:

```
/updatearl <your_new_arl>
```

The bot will:
1. Re-login to deemix immediately with the new ARL
2. Update the ARL in your compose file on disk
3. Trigger a container rebuild so the new ARL persists across restarts

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
