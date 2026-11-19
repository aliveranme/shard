FROM node:26.13.1-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY scripts/git-origin.mjs ./git-origin.mjs
ENV SHARD_GIT_BRIDGE_ONLY=1 \
    SHARD_GIT_HOST=0.0.0.0
USER 1000:1000
EXPOSE 8793
ENTRYPOINT ["node", "/app/git-origin.mjs"]
