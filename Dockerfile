# Preload dependencies,
# used to speed up repeated builds and reduce traffic consumption of libraries
FROM rust:latest AS stage-deps

RUN apt update && apt install -y clang
RUN rustup target add wasm32-unknown-unknown
RUN rustup target add wasm32-wasi
RUN cargo install cargo-make
RUN cargo install wasm-bindgen-cli@0.2.95

COPY ./Cargo.toml /home/Cargo.toml
RUN cargo new --name server /home/packages/server
COPY ./packages/server/Cargo.toml /home/packages/server/Cargo.toml
RUN cargo new --lib --name client /home/packages/client
COPY ./packages/client/Cargo.toml /home/packages/client/Cargo.toml
RUN cargo new --lib --name database /home/packages/database
COPY ./packages/database/Cargo.toml /home/packages/database/Cargo.toml

ENV ROOT_DIR=/home/res
WORKDIR /home
RUN cargo fetch

COPY ./packages/server /home/packages/server
COPY ./packages/client /home/packages/client
COPY ./packages/database /home/packages/database

COPY ./res/languages /home/res/languages
COPY ./res/website /home/res/website
COPY ./res/Config.default.toml /home/res/Config.default.toml

# Stage 1 for client build, used to compile wasm file
FROM stage-deps AS stage-client-build1

WORKDIR /home
RUN cargo build --offline --package _client --target wasm32-unknown-unknown --release

# Stage 2 for client build, used to process wasm file for browser platform
FROM stage-deps AS stage-client-build2

COPY --from=stage-client-build1 /home/target/wasm32-unknown-unknown/release/_client.wasm /home/client.wasm
WORKDIR /home
RUN wasm-bindgen\
  --out-dir /home/dist\
  --out-name client\
  --target no-modules\
  --no-typescript\
  --no-modules-global wasm_vendor_entry\
  client.wasm

# Stage 1 for server build, used to compile server program
FROM stage-deps AS stage-server-build1

WORKDIR /home
RUN cargo build --offline --package _server --release

# Stage 2 for server build, used to integrate the build result of client and generate the final image
FROM ubuntu:latest AS stage-server-build2

RUN apt update && apt install -y openssl

COPY ./res/website /home/website
COPY --from=stage-client-build2 /home/dist/client_bg.wasm /home/client_bg.wasm
COPY --from=stage-client-build2 /home/dist/client.js /home/client.js
COPY --from=stage-server-build1 /home/target/release/_server /home/a

ENV ROOT_DIR=/home
WORKDIR /home
ENTRYPOINT [ "./a" ]
EXPOSE 80
