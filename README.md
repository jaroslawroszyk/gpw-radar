# gpu-radar

Bot na Telegrama, który pilnuje za mnie GPW - skanuje ESPI i kilka innych RSS-ów,
łapie newsy dotyczące spółek z mojego portfela i wysyła alert, zamiast żebym
sam co godzinę odświeżał Bankiera.

Do tego dorzuciłem po drodze: kursy (Yahoo + fallback na Bankier.pl), alerty
cenowe, wykresy z RSI/SMA, wykrywanie transakcji insiderów (art. 19 MAR) i
krótkie podsumowania AI (Groq/Llama) na komunikatach ESPI.

## Uruchomienie

```
cp .env.example .env   # TELOXIDE_TOKEN, CHAT_ID, opcjonalnie GROQ_API_KEY
cargo run
```

Lista śledzonych spółek i źródła RSS są w `config.toml`. Spółki można też
dodawać/usuwać z Telegrama (`/dodaj`, `/usun`).

## Komendy

`/status`, `/portfel`, `/dodaj`, `/usun`, `/analiza`, `/wykres`, `/alert`, `/eksport`

Pełna lista i opisy - komenda `/help`.

## Docker

```
docker build -t stock-news-bot .
docker run -e TELOXIDE_TOKEN=... -e CHAT_ID=... stock-news-bot
```

Health check i metryki Prometheusa na porcie 8080 (`/health`, `/metrics`).
>>>>>>> 21aa893 (docs: rewrite README)
