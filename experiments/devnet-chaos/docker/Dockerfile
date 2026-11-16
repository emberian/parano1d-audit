FROM rust:1.96-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang libclang-dev pkg-config iproute2 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
