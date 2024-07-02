FROM rust:latest as builder
WORKDIR /reflux
COPY . .
RUN cargo install --path bin/reflux --profile release

FROM debian:latest
RUN apt-get update
RUN apt-get upgrade -y
RUN apt-get install -y libssl-dev ca-certificates
COPY --from=builder /usr/local/cargo/bin/reflux /app/reflux
