import { timingSafeEqual } from "node:crypto";

const ID = /^shard-[A-Za-z0-9._-]{1,122}$/;
const PAGE_SIZE = 100;
const headers = { "Cache-Control": "no-store" };

export default {
  async fetch(request, env) {
    const token = env.SHARD_R2_RECOVERY_TOKEN;
    const authorization = request.headers.get("Authorization") ?? "";
    if (typeof token !== "string" || !/^[a-f0-9]{64}$/.test(token)) {
      return new Response(null, { status: 404, headers });
    }
    const encoder = new TextEncoder();
    const expected = encoder.encode(`Bearer ${token}`);
    const supplied = encoder.encode(authorization);
    if (expected.length !== supplied.length || !timingSafeEqual(expected, supplied)) {
      return new Response(null, { status: 401, headers });
    }

    const url = new URL(request.url);
    if (url.pathname === "/repositories" && request.method === "GET") {
      const after = url.searchParams.get("after") ?? "";
      if (after && !ID.test(after)) return new Response(null, { status: 400, headers });
      const { results } = await env.DB.prepare(
        "SELECT artifacts_repo AS storage_id FROM repos WHERE git_backend = 'r2' AND artifacts_repo > ? ORDER BY artifacts_repo LIMIT ?",
      ).bind(after, PAGE_SIZE + 1).all();
      const repositories = results.slice(0, PAGE_SIZE).map(row => row.storage_id);
      const next = results.length > PAGE_SIZE ? repositories.at(-1) : null;
      return Response.json({ repositories, next }, { headers });
    }

    const id = url.pathname.slice("/repositories/".length);
    if (!url.pathname.startsWith("/repositories/") || !ID.test(id) || url.search ||
        !["GET", "PUT"].includes(request.method)) {
      return new Response(null, { status: 404, headers });
    }
    const repo = await env.DB.prepare("SELECT 1 FROM repos WHERE git_backend = 'r2' AND artifacts_repo = ?").bind(id).first();
    if (!repo) return new Response(null, { status: 404, headers });
    return env.GIT_R2_REPOS.getByName(id).fetch(`https://internal/v1/r2/${id}/recovery`, {
      method: request.method,
      body: request.body,
      duplex: "half",
    });
  },
};
