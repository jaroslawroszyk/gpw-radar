# Krok 1: Budowanie binarki przy użyciu najnowszej stabilnej wersji Rusta
FROM rust:latest as builder
WORKDIR /app

# Kopiujemy pliki projektu
COPY . .

# Kompilujemy wersję produkcyjną
RUN cargo build --release

# Krok 2: Lekki obraz uruchomieniowy
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Kopiujemy skompilowaną binarkę oraz konfigurację
COPY --from=builder /app/target/release/stock_news_bot /app/
COPY config.toml /app/

# Otwieramy port dla Health Checków
EXPOSE 8080

CMD ["./stock_news_bot"]