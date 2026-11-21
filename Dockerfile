# Preload dependencies,
# used to speed up repeated builds and reduce traffic consumption of libraries
FROM rust:latest AS stage-deps

RUN apt update && apt install -y clang

COPY ./Cargo.toml /home/Cargo.toml
COPY ./Cargo.lock /home/Cargo.lock
RUN cargo new --name server /home/packages/server
COPY ./packages/server/Cargo.toml /home/packages/server/Cargo.toml
RUN cargo new --lib --name _database /home/packages/database
COPY ./packages/database/Cargo.toml /home/packages/database/Cargo.toml
RUN cargo new --lib --name _functions /home/packages/functions
COPY ./packages/functions/Cargo.toml /home/packages/functions/Cargo.toml
RUN cargo new --lib --name _types /home/packages/types
COPY ./packages/types/Cargo.toml /home/packages/types/Cargo.toml

ENV ROOT_DIR=/home/res
WORKDIR /home
RUN cargo fetch --locked

COPY ./packages/server /home/packages/server
COPY ./packages/database /home/packages/database
COPY ./packages/functions /home/packages/functions
COPY ./packages/types /home/packages/types

COPY ./res/languages /home/res/languages
COPY ./res/website /home/res/website
COPY ./res/Config.default.toml /home/res/Config.default.toml

# Compile the server binary (the browser client lane is retired; the hikari
# webui wave will re-introduce a frontend build stage).
FROM stage-deps AS stage-server-build

WORKDIR /home
RUN cargo build --offline --locked --package _server --release

# Runtime image with the static website and the server binary.
FROM ubuntu:latest AS stage-runtime

RUN apt update && apt install -y openssl

COPY ./res/website /home/website
COPY --from=stage-server-build /home/target/release/_server /home/a

ENV ROOT_DIR=/home
WORKDIR /home
ENTRYPOINT [ "./a" ]
EXPOSE 80
