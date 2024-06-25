FROM rust:latest as builder
WORKDIR /reflux
COPY . .
RUN cargo install --path bin/reflux --profile release

FROM debian:bullseye-slim
COPY --from=builder /usr/local/cargo/bin/reflux /app/reflux

