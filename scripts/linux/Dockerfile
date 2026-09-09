# Ubuntu 24.04 multi-platform index, published 2026-08-10.
FROM ubuntu:24.04@sha256:33ceb71981b602c1a7443a53469e4dba065f7503eab3078a2d7a57a2ab987517
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl git pkg-config \
    python3 ripgrep rsync procps locales xz-utils unzip zsh \
    libxkbcommon-dev libxkbcommon-x11-dev mesa-vulkan-drivers \
    fontconfig fonts-dejavu-core xvfb xauth xdotool x11-xkb-utils \
    x11-utils openbox xcompmgr twm \
    && rm -rf /var/lib/apt/lists/* \
    && locale-gen en_US.UTF-8

# Pin both platform archives instead of executing the moving install script.
RUN case "$TARGETARCH" in \
      arm64) mise_arch=arm64; mise_sha=98d2ea7b82dd966afdb8a9f4e9edbca771acf2a30d2842bfc0efdb7b61c886a3 ;; \
      amd64) mise_arch=x64; mise_sha=063dda9149ab6be53da877c2d176afe0eac68e64cf8ca295bd0528720701c65d ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSL "https://github.com/jdx/mise/releases/download/v2026.9.1/mise-v2026.9.1-linux-${mise_arch}.tar.gz" -o /tmp/mise.tar.gz \
    && echo "$mise_sha  /tmp/mise.tar.gz" | sha256sum --check \
    && tar -xzf /tmp/mise.tar.gz -C /tmp \
    && install /tmp/mise/bin/mise /usr/local/bin/mise \
    && rm -rf /tmp/mise /tmp/mise.tar.gz

ENV MISE_AUTO_INSTALL=false MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/huterm-tools:/workspace \
    LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 SHELL=/bin/bash
WORKDIR /opt/huterm-tools
COPY mise.toml mise.lock rust-toolchain.toml ./
RUN mise install rust bun zig \
    && ln -s /cache/cargo-registry /root/.cargo/registry \
    && ln -s /cache/cargo-git /root/.cargo/git

# Limit concurrent rustc memory use in a desktop Docker VM. Callers can
# override this per command with linux:exec -- env CARGO_BUILD_JOBS=...
ENV CARGO_BUILD_JOBS=4 ZIG_GLOBAL_CACHE_DIR=/cache/zig \
    BUN_INSTALL_CACHE_DIR=/cache/bun MISE_CACHE_DIR=/cache/mise
COPY entrypoint.sh /usr/local/bin/huterm-linux
WORKDIR /workspace
ENTRYPOINT ["/bin/bash", "/usr/local/bin/huterm-linux"]
