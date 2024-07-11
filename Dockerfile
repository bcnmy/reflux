FROM --platform=linux/amd64 rust:1.79.0-slim-bullseye AS builder

ENV APP_PATH=/reflux

RUN apt-get update  && apt-get install -y --no-install-recommends libssl-dev pkg-config

RUN USER=root cargo new --bin reflux

WORKDIR ${APP_PATH}

COPY . ${APP_PATH}

COPY Cargo.toml Cargo.lock ${APP_PATH}

RUN cargo build --release --manifest-path ${APP_PATH}/Cargo.toml



# Second stage
FROM --platform=linux/amd64 debian:bullseye-slim as execution


# Tini allows us to avoid several Docker edge cases, see https://github.com/krallin/tini.
# NOTE: See https://github.com/hexops/dockerfile#is-tini-still-required-in-2020-i-thought-docker-added-it-natively

RUN apt-get update && apt-get install -y --no-install-recommends \
    tini libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security purposes.
#
# UIDs below 10,000 are a security risk, as a container breakout could result
# in the container being ran as a more privileged user on the host kernel with
# the same UID.
#
# Static GID/UID is also useful for chown'ing files outside the container where
# such a user does not exist.
RUN addgroup --gid 10001 --system nonroot \
  && adduser  --uid 10000 --system --ingroup nonroot --home /home/nonroot nonroot


WORKDIR /home/nonroot/reflux

COPY --from=builder --chown=10000:10001 /reflux/target/release/reflux /usr/local/bin/

USER nonroot

ENTRYPOINT ["/usr/bin/tini", "--"]

# Run the binary
CMD ["reflux"]
