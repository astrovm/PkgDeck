FROM docker.io/library/ubuntu:26.04@sha256:513c074113a871b51a8d16ab445c88779d6452d937a164fb5cc479f32668a41d
ENV DEBIAN_FRONTEND=noninteractive container=podman
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates \
    && printf 'container\n' > /etc/pkgdeck-disposable-container
COPY scripts/vm/prepare.py /opt/pkgdeck/prepare.py
RUN python3 -u /opt/pkgdeck/prepare.py --container
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core libgl1 libegl1 libopengl0 && rm -rf /var/lib/apt/lists/*
