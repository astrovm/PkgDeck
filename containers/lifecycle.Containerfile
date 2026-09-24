FROM docker.io/library/ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78
ENV DEBIAN_FRONTEND=noninteractive container=podman
RUN sed -i 's|http://security.ubuntu.com/ubuntu/|http://azure.archive.ubuntu.com/ubuntu/|; s|http://archive.ubuntu.com/ubuntu/|http://azure.archive.ubuntu.com/ubuntu/|' /etc/apt/sources.list.d/ubuntu.sources
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && printf 'container\n' > /etc/pkgdeck-disposable-container
COPY scripts/vm/prepare.sh /opt/pkgdeck/prepare.sh
RUN bash /opt/pkgdeck/prepare.sh --container
RUN apt-get update && apt-get install -y --no-install-recommends xvfb xdotool fonts-dejavu-core libgl1 libegl1 libopengl0 && rm -rf /var/lib/apt/lists/*
