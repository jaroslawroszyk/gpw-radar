# gpu-radar

A Telegram bot that keeps an eye on the WSE for me – it scans ESPI and a few other RSS feeds, catches news about companies in my portfolio, and sends alerts instead of me refreshing Bankier every hour.

I've also added: rates (Yahoo + fallback on Bankier.pl), price alerts, RSI/SMA charts, insider transaction detection (Article 19 of MAR), and short AI summaries (Groq/Llama) on ESPI announcements.

## Activation

```
cp .env.example .env   # TELOXIDE_TOKEN, CHAT_ID, optional GROQ_API_KEY
cargo run
```

The list of followed companies and RSS feeds is in `config.toml`. Companies can also be added/removed from Telegram (`/add`, `/remove`).

## Commands

`/status`, `/wallet`, `/add`, `/remove`, `/analysis`, `/chart`, `/alert`, `/export`

Full list and descriptions - `/help` command.

## Docker

```
docker build -t stock-news-bot .
docker run -e TELOXIDE_TOKEN=... -e CHAT_ID=... stock-news-bot
```

Prometheus health check and metrics on port 8080 (`/health`, `/metrics`).

## Deploy on Render.com

You can host this bot on Render without keeping your PC on all the time.

1. Create a new Web Service in Render and connect this repository.
2. Use the existing Dockerfile (recommended) or set the build/start commands manually.
3. Add these environment variables:
   - `TELOXIDE_TOKEN`
   - `CHAT_ID`
   - `GROQ_API_KEY` (optional)
4. Render will expose the app on port 8080; the bot already serves health checks at `/health` and `/metrics`.

Example Render settings:
- Build Command: `docker build -t stock-news-bot .`
- Start Command: `docker run -e TELOXIDE_TOKEN=$TELOXIDE_TOKEN -e CHAT_ID=$CHAT_ID -e GROQ_API_KEY=$GROQ_API_KEY stock-news-bot`

## Keep it alive with cron-job.org

Render's free tier can sleep after inactivity, so the bot may stop responding for a while. To avoid that, you can wake it periodically with cron-job.org (or another similar scheduler).

1. Sign in to https://console.cron-job.org/dashboard
2. Create a new job that calls your health endpoint every 5–10 minutes.
3. Use a URL like:

```text
https://YOUR-APP.onrender.com/health
```

Example cron expression:

```text
*/5 * * * *
```

This is a simple way to keep the service warm without running your own computer 24/7.