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
    eatmydata \
    pigz \
    aria2 \
    moreutils \
    # AArch64 specific packages
    gcc-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    qemu-system-arm \
    # x86_64 specific packages
    qemu-system

RUN apt-get clean && \
    rm -rf /var/lib/apt/lists/*
