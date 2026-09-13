FROM debian:bookworm-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends quota rpcbind nfs-kernel-server nfs-common e2fsprogs util-linux python3 && rm -rf /var/lib/apt/lists/*
COPY entrypoint.sh /entrypoint.sh
COPY verify.py /verify.py
ENTRYPOINT ["/bin/sh", "/entrypoint.sh"]
