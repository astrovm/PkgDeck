FROM docker.io/library/ubuntu:26.04@sha256:2e27ff302056b2b96df4a524ce48e72c595abfe3fd7a3d923adf8e782e16896f
ENV DEBIAN_FRONTEND=noninteractive container=podman
COPY scripts/ci/apt-mirrors.sh /opt/pkgdeck/apt-mirrors.sh
RUN bash /opt/pkgdeck/apt-mirrors.sh
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && printf 'container\n' > /etc/pkgdeck-disposable-container
COPY scripts/vm/prepare.sh /opt/pkgdeck/prepare.sh
RUN bash /opt/pkgdeck/prepare.sh --container
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core libgl1 libegl1 libopengl0 \
    libgl1-mesa-dri libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6 libxkbcommon0 libxkbcommon-x11-0 && rm -rf /var/lib/apt/lists/*
