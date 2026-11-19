FROM node:26.13.1-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates fuse3 git rclone \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY scripts/git-origin.mjs scripts/git-r2-origin.mjs ./
EXPOSE 8793
ENTRYPOINT ["node", "/app/git-r2-origin.mjs"]
