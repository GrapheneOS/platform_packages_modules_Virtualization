# The Dockerfile should be synced with `install_prerequisites` in `build.sh` and `build_custom_kernel.sh`
# Use Ubuntu 22.04 as the base image
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    software-properties-common \
    ca-certificates \
    curl \
    gnupg

RUN apt-get install -y --no-install-recommends \
    # Base packages
    apt-utils \
    automake \
    binfmt-support \
    build-essential \
    cmake \
    debsums \
    dosfstools \
    fdisk \
    git \
    libjson-c-dev \
    libtool \
    libwebsockets-dev \
    make \
    protobuf-compiler \
    python3 \
    python3-libcloud \
    python3-marshmallow \
    python3-pytest \
    python3-yaml \
    qemu-user-static \
    qemu-utils \
    sudo \
    udev \
    wget \
    jq \
    # AArch64 specific packages
    gcc-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    qemu-system-arm \
    # x86_64 specific packages
    qemu-system \
    # Cloud-init optional package
    genisoimage

RUN apt-get clean && \
    rm -rf /var/lib/apt/lists/*

ENV PATH="/root/.cargo/bin:${PATH}"

##
# Install Rust and Tooling
##
# This section installs Rust and adds support for both x86_64 and aarch64
# targets to make the builder image more versatile.
RUN set -eux; \
    # Install rustup
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
    \
    # Install targets for both architectures
    rustup target add aarch64-unknown-linux-gnu; \
    rustup target add x86_64-unknown-linux-gnu; \
    \
    # Install common cargo tools
    cargo install cargo-license --version 0.6.1; \
    cargo install cargo-deb --version 3.3.0;
