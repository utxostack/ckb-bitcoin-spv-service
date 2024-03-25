FROM rust:latest AS builder

WORKDIR /app
COPY . /app

RUN apt-get update && apt-get install --no-install-recommends -y clang
RUN cargo build --release

FROM ubuntu:latest

ARG DEBCONF_NOWARNINGS="yes"
ARG DEBIAN_FRONTEND noninteractive
ARG DEBCONF_NONINTERACTIVE_SEEN true

RUN apt-get update \
 && apt-get install --no-install-recommends -y \
    tini \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

COPY --from=builder /app/target/release/ckb-bitcoin-spv-service /usr/local/bin/ckb-bitcoin-spv-service

RUN chmod a+x /usr/local/bin/ckb-bitcoin-spv-service

ENTRYPOINT [ "tini", "--"]
CMD ["ckb-bitcoin-spv-service", "--help"]
