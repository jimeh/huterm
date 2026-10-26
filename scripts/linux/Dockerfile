# Ubuntu 22.04 multi-platform index, published 2026-09-01.
FROM ubuntu:22.04@sha256:829f6df217bcbae2b371026e81711d1a787c61b2967ad09d015063663ebafbf7
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl file git pkg-config \
    python3 ripgrep rsync procps locales ncurses-bin patchelf pax-utils tmux xz-utils unzip zsh \
    libxkbcommon-dev libxkbcommon-x11-dev mesa-vulkan-drivers \
    fontconfig fonts-dejavu-core xvfb xauth xdotool x11-xkb-utils \
    x11-utils openbox xcompmgr xclip twm desktop-file-utils appstream \
    && rm -rf /var/lib/apt/lists/* \
    && locale-gen en_US.UTF-8

# Pin both platform archives instead of executing the moving install script.
RUN case "$TARGETARCH" in \
      arm64) mise_arch=arm64; mise_sha=e6db9cd4cd45dfa0323080badb23db59951ce3cf077f54c691066735220085c6 ;; \
      amd64) mise_arch=x64; mise_sha=aa5f8fbbb942296bbeea3769abf56c6f4ee37da4d203f0044b227cac4d929a87 ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSL "https://github.com/jdx/mise/releases/download/v2026.10.12/mise-v2026.10.12-linux-${mise_arch}.tar.gz" -o /tmp/mise.tar.gz \
    && echo "$mise_sha  /tmp/mise.tar.gz" | sha256sum --check \
    && tar -xzf /tmp/mise.tar.gz -C /tmp \
    && install /tmp/mise/bin/mise /usr/local/bin/mise \
    && rm -rf /tmp/mise /tmp/mise.tar.gz

# Commands run as an unprivileged user, as on CI runners: root ignores file
# permissions, which tests of unreadable or unusable paths depend on. The
# mount points belong to that user, so Docker seeds new volumes with it.
RUN useradd --create-home --shell /bin/bash huterm \
    && install -d -o huterm -g huterm /opt/huterm-tools /workspace /cache

ENV MISE_AUTO_INSTALL=false MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS=/opt/huterm-tools:/workspace \
    LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 SHELL=/bin/bash
USER huterm
WORKDIR /opt/huterm-tools
COPY --chown=huterm:huterm mise.toml mise.lock rust-toolchain.toml ./
RUN mise install rust bun zig \
    && ln -s /cache/cargo-registry "$HOME/.cargo/registry" \
    && ln -s /cache/cargo-git "$HOME/.cargo/git"

# Limit concurrent rustc memory use in a desktop Docker VM. Callers can
# override this per command with linux:exec -- env CARGO_BUILD_JOBS=...
ENV CARGO_BUILD_JOBS=4 ZIG_GLOBAL_CACHE_DIR=/cache/zig \
    BUN_INSTALL_CACHE_DIR=/cache/bun MISE_CACHE_DIR=/cache/mise
COPY entrypoint.sh /usr/local/bin/huterm-linux
WORKDIR /workspace
# The entrypoint starts as root to copy the source and fix volume ownership,
# then runs the command as that user.
USER root
ENTRYPOINT ["/bin/bash", "/usr/local/bin/huterm-linux"]
