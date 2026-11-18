# Build-only recipe; no invocation or image publication is performed by this file.
# Build context must be the canonical SIS source or a verified allowlist copy.
FROM rust:1.91 AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY migrations/ ./migrations/
RUN cargo build --release --locked --bin sis-service

# A minimal ffmpeg for tahfiz clips: WebM/Ogg in, Opus/Vorbis decoded, AAC in MP4 out, local files only.
# It adds about 2 MB to the image and links only libc and libm (the Debian package would add about 380 MB).
# Upgrade: new version and the sha256 of its tarball (signed by the FFmpeg release key) together.
FROM debian:trixie-20261104-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS ffmpeg
ARG FFMPEG_VERSION=9.0.2
ARG FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
RUN apt-get update && apt-get install -y --no-install-recommends build-essential ca-certificates curl xz-utils && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN curl -fsSLo ffmpeg.tar.xz "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz" \
 && echo "${FFMPEG_SHA256}  ffmpeg.tar.xz" | sha256sum -c - \
 && tar -xJf ffmpeg.tar.xz --strip-components=1 \
 && ./configure --prefix=/opt/ffmpeg --disable-everything --disable-autodetect --disable-doc --disable-debug \
      --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --disable-swscale --disable-x86asm \
      --enable-small --enable-protocol=file --enable-demuxer=matroska,ogg --enable-parser=opus,vorbis \
      --enable-decoder=opus,vorbis --enable-encoder=aac --enable-muxer=mp4,ipod \
      --enable-filter=aresample,aformat,anull \
 && make -j"$(nproc)" && make install && strip /opt/ffmpeg/bin/ffmpeg

FROM debian:trixie-20261104-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3t64 && rm -rf /var/lib/apt/lists/*
COPY --from=ffmpeg /opt/ffmpeg/bin/ffmpeg /usr/local/bin/ffmpeg
COPY --from=builder /app/target/release/sis-service ./sis-service
EXPOSE 8081
CMD ["./sis-service"]

ARG SOURCE_REPOSITORY
ARG SOURCE_REVISION
ARG SOURCE_TREE_SHA256
LABEL org.opencontainers.image.source=$SOURCE_REPOSITORY \
      org.opencontainers.image.revision=$SOURCE_REVISION \
      tech.cybe.digital-school.source-tree-sha256=$SOURCE_TREE_SHA256
