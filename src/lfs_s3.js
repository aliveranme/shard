import { DOMParser } from "@xmldom/xmldom";
import { AwsV4Signer } from "aws4fetch";

const MiB = 1024 ** 2;
const maxPartBytes = 5 * 1024 ** 3 - 5 * MiB;
const maxObjectBytes = 5 * 1024 ** 4 - 5 * 1024 ** 3;
const maxParts = 10_000;
const copyConcurrency = 4;
const binaryContentType = "application/octet-stream";
const createOnly = { "if-none-match": "*" };
const localS3Path = "/cdn-cgi/local/r2/s3";
const maxXmlBytes = MiB;
const parser = new DOMParser({
  locator: false,
  onError() {
    throw new Error("Invalid S3 XML");
  },
});

function encodeSegment(value) {
  return encodeURIComponent(value).replace(/[!'()*]/g, (char) =>
    `%${char.charCodeAt(0).toString(16).toUpperCase()}`);
}

function objectPath(bucket, key) {
  const segments = key.split("/");
  if (!bucket || !key || new TextEncoder().encode(key).length > 1024 ||
      bucket === "." || bucket === ".." || segments.some((segment) => segment === "." || segment === "..")) {
    throw new Error("Invalid R2 object path");
  }
  return `/${encodeSegment(bucket)}/${segments.map(encodeSegment).join("/")}`;
}

function s3Client(endpoint, accessKeyId, secretAccessKey) {
  let url;
  try {
    url = new URL(endpoint);
  } catch {
    throw new Error("Invalid R2 S3 endpoint");
  }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if ((url.protocol !== "https:" && (url.protocol !== "http:" || !loopback)) ||
      url.username || url.password || url.search || url.hash) {
    throw new Error("Invalid R2 S3 endpoint");
  }
  const prefix = url.pathname.replace(/\/+$/, "");
  return {
    url,
    prefix,
    isMiniflare: loopback && prefix === localS3Path,
    credentials: { accessKeyId, secretAccessKey, service: "s3", region: "auto", cache: new Map() },
  };
}

function atObject(client, path) {
  const url = new URL(client.url);
  url.pathname = client.prefix + path;
  return url;
}

function withQuery(url, query) {
  const result = new URL(url);
  result.search = new URLSearchParams(query).toString();
  return result;
}

function within(value, min, max) {
  return Number.isSafeInteger(value) && value >= min && value <= max;
}

function decimal(text) {
  return /^\d+$/.test(text) ? Number(text) : NaN;
}

function quotedEtag(value) {
  const etag = value.startsWith('"') ? value : `"${value}"`;
  if (!/^"[\x21\x23-\x7e]+"$/.test(etag)) throw new Error("Invalid R2 ETag");
  return etag;
}

async function sign(client, url, options) {
  try {
    return await new AwsV4Signer({ ...client.credentials, url: url.href, allHeaders: true, ...options }).sign();
  } catch {
    throw new Error("Could not sign R2 S3 request");
  }
}

export async function lfsPresign(endpoint, bucket, accessKeyId, secretAccessKey, key, uploadId, partNumber, length, checksum) {
  const part = Number(partNumber);
  const bytes = Number(length);
  const multipart = uploadId !== "";
  if (!within(bytes, 0, maxPartBytes) || (multipart ? !within(part, 1, maxParts) : part !== 0)) {
    throw new Error("Invalid R2 upload length or part number");
  }
  const client = s3Client(endpoint, accessKeyId, secretAccessKey);
  const url = atObject(client, objectPath(bucket, key));
  const header = { "content-length": String(bytes), "content-type": binaryContentType };
  if (multipart) {
    url.searchParams.set("uploadId", uploadId);
    url.searchParams.set("partNumber", String(part));
  } else {
    if (!/^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$/.test(checksum)) {
      throw new Error("Invalid R2 upload SHA-256 checksum");
    }
    header["x-amz-checksum-sha256"] = checksum;
    header["if-none-match"] = "*";
  }
  url.searchParams.set("X-Amz-Expires", "3600");
  const signed = await sign(client, url, { method: "PUT", headers: header, signQuery: true });
  return { href: signed.url.href, header };
}

class S3Error extends Error {
  constructor(operation, status, code) {
    super(`R2 ${operation} failed (HTTP ${status}${code ? `, ${code}` : ""})`);
    this.operation = operation;
    this.status = status;
    this.code = code;
  }
}

async function readXml(response) {
  if (!response.body) return null;
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let xml = "";
  let bytes = 0;
  let done = false;
  try {
    for (;;) {
      const next = await reader.read();
      if (next.done) {
        done = true;
        break;
      }
      bytes += next.value.byteLength;
      if (bytes > maxXmlBytes) throw new Error("R2 XML response exceeds size limit");
      xml += decoder.decode(next.value, { stream: true });
    }
    xml += decoder.decode();
  } finally {
    if (!done) await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  const text = xml.trimStart();
  if (!text) return null;
  const document = parser.parseFromString(text, "application/xml");
  if (document.doctype) throw new Error("Unexpected S3 XML doctype");
  return document.documentElement;
}

function field(root, name) {
  for (let child = root.firstChild; child; child = child.nextSibling) {
    if (child.localName === name) return child.textContent.trim();
  }
  return "";
}

async function s3Request(client, operation, url, { method, headers, body, signal, xmlRoot }) {
  const signed = await sign(client, url, { method, headers, body });
  let response;
  try {
    response = await fetch(signed.url, { method, headers: signed.headers, body, signal, redirect: "manual" });
  } catch {
    throw new Error(`R2 ${operation} ${signal.aborted ? "timed out" : "request failed"}`);
  }
  if ((!xmlRoot && response.ok) || (response.status >= 300 && response.status < 400)) {
    await response.body?.cancel().catch(() => {});
    if (response.ok) return response;
    throw new S3Error(operation, response.status, "");
  }
  let root;
  try {
    root = await readXml(response);
  } catch {
    if (response.ok || signal.aborted) {
      throw new Error(`R2 ${operation} ${signal.aborted ? "timed out" : "returned invalid XML"}`);
    }
  }
  if (!response.ok || root?.localName === "Error") {
    const code = root?.localName === "Error" ? field(root, "Code") : "";
    throw new S3Error(operation, response.status, /^[A-Za-z][A-Za-z0-9]{0,63}$/.test(code) ? code : "");
  }
  if (root?.localName !== xmlRoot) throw new Error(`R2 ${operation} returned unexpected XML`);
  return root;
}

function isPreconditionFailure(error) {
  return error instanceof S3Error && (error.code === "PreconditionFailed" || (error.status === 412 && !error.code));
}

async function checkSource(client, url, size, etag, signal) {
  const response = await s3Request(client, "HeadObject", url, { method: "HEAD", headers: { "if-match": etag }, signal });
  if (decimal(response.headers.get("content-length")) !== size || response.headers.get("etag") !== etag) {
    throw new Error("R2 copy source differs from the verified object");
  }
}

function escapeXml(value) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

async function copyMultipart(client, destination, source, copySource, size, etag, signal, timeoutMs) {
  let initiated;
  try {
    initiated = await s3Request(client, "CreateMultipartUpload", withQuery(destination, { uploads: "" }), {
      method: "POST",
      headers: { ...createOnly, "content-type": binaryContentType },
      signal,
      xmlRoot: "InitiateMultipartUploadResult",
    });
  } catch (error) {
    if (isPreconditionFailure(error)) return false;
    throw error;
  }
  const uploadId = field(initiated, "UploadId");
  if (!uploadId) throw new Error("R2 did not return a copy upload ID");
  const upload = withQuery(destination, { uploadId });

  try {
    const partCount = Math.ceil(size / maxPartBytes);
    const parts = new Array(partCount);
    for (let first = 1; first <= partCount; first += copyConcurrency) {
      const group = [];
      for (let partNumber = first; partNumber <= partCount && partNumber < first + copyConcurrency; partNumber++) {
        const url = withQuery(destination, { uploadId, partNumber });
        const start = (partNumber - 1) * maxPartBytes;
        const end = Math.min(start + maxPartBytes, size) - 1;
        group.push(s3Request(client, "UploadPartCopy", url, {
          method: "PUT",
          headers: { "x-amz-copy-source": copySource, "x-amz-copy-source-range": `bytes=${start}-${end}` },
          signal,
          xmlRoot: "CopyPartResult",
        }));
      }
      // Wait for outstanding copies before an abort can race them.
      const results = await Promise.allSettled(group);
      for (let index = 0; index < results.length; index++) {
        const result = results[index];
        if (result.status === "rejected") throw result.reason;
        const partNumber = first + index;
        const partEtag = escapeXml(quotedEtag(field(result.value, "ETag")));
        parts[partNumber - 1] = `<Part><PartNumber>${partNumber}</PartNumber><ETag>${partEtag}</ETag></Part>`;
      }
    }
    // R2 UploadPartCopy lacks a source condition. The caller guarantees the source
    // is immutable, and these checks bracket its use by the copy operations.
    await checkSource(client, source, size, etag, signal);
    const result = await s3Request(client, "CompleteMultipartUpload", upload, {
      method: "POST",
      headers: { ...createOnly, "content-type": "application/xml" },
      body: `<CompleteMultipartUpload>${parts.join("")}</CompleteMultipartUpload>`,
      signal,
      xmlRoot: "CompleteMultipartUploadResult",
    });
    quotedEtag(field(result, "ETag"));
    return true;
  } catch (error) {
    // The copy may already have timed out, so cleanup needs its own signal.
    const cleanup = AbortSignal.timeout(Math.min(timeoutMs, 10_000));
    await s3Request(client, "AbortMultipartUpload", upload, { method: "DELETE", signal: cleanup }).catch(() => {});
    if (error.operation === "CompleteMultipartUpload" && isPreconditionFailure(error)) return false;
    throw error;
  }
}

export async function lfsCopy(endpoint, bucket, accessKeyId, secretAccessKey, source, target, size, etag, timeoutMs) {
  const bytes = Number(size);
  const timeout = Number(timeoutMs);
  if (!within(bytes, maxPartBytes + 1, maxObjectBytes) || !within(timeout, 1, 2 ** 31 - 1)) {
    throw new Error("Invalid R2 copy size or timeout");
  }
  const client = s3Client(endpoint, accessKeyId, secretAccessKey);
  // Miniflare currently ignores destination conditions on CompleteMultipartUpload.
  if (client.isMiniflare) throw new Error("Local R2 S3 does not support atomic create-only multipart completion");
  const copySource = objectPath(bucket, source);
  const sourceUrl = atObject(client, copySource);
  const destination = atObject(client, objectPath(bucket, target));
  const sourceEtag = quotedEtag(etag);
  const signal = AbortSignal.timeout(timeout);
  await checkSource(client, sourceUrl, bytes, sourceEtag, signal);
  return copyMultipart(client, destination, sourceUrl, copySource, bytes, sourceEtag, signal, timeout);
}

async function listUploadParts(client, bucket, key, uploadId) {
  const parts = [];
  const signal = AbortSignal.timeout(25_000);
  const url = withQuery(atObject(client, objectPath(bucket, key)), { uploadId, "max-parts": "1000" });
  let marker = 0;
  for (;;) {
    if (marker) url.searchParams.set("part-number-marker", String(marker));
    const page = await s3Request(client, "ListParts", url, { method: "GET", signal, xmlRoot: "ListPartsResult" });
    let last = marker;
    for (let element = page.firstChild; element; element = element.nextSibling) {
      if (element.localName !== "Part") continue;
      const partNumber = decimal(field(element, "PartNumber"));
      const size = decimal(field(element, "Size"));
      const etag = quotedEtag(field(element, "ETag")).slice(1, -1);
      if (!within(partNumber, last + 1, maxParts) || !within(size, 1, maxPartBytes) || etag.length > 256) {
        throw new Error("R2 returned an invalid upload part");
      }
      parts.push({ partNumber, size, etag });
      last = partNumber;
    }
    if (field(page, "IsTruncated") === "false") return parts;
    const next = decimal(field(page, "NextPartNumberMarker"));
    if (field(page, "IsTruncated") !== "true" || next !== last || !within(next, marker + 1, maxParts - 1)) {
      throw new Error("R2 returned an invalid part listing cursor");
    }
    marker = next;
  }
}

export async function lfsListParts(endpoint, bucket, accessKeyId, secretAccessKey, key, uploadId) {
  if (!uploadId) throw new Error("Invalid R2 upload ID");
  const client = s3Client(endpoint, accessKeyId, secretAccessKey);
  try {
    return { status: "ok", parts: await listUploadParts(client, bucket, key, uploadId) };
  } catch (error) {
    if (error instanceof S3Error && (error.code === "NoSuchUpload" || error.status === 404)) {
      return { status: "missing", parts: [] };
    }
    throw error;
  }
}

// Git uploads reconcile against R2 itself, including parts whose PUT response was lost.
export function gitR2S3(env) {
  const endpoint = env.SHARD_GIT_R2_ENDPOINT;
  const bucket = env.SHARD_GIT_R2_BUCKET;
  const access = env.SHARD_GIT_R2_ACCESS_KEY_ID;
  const secret = env.SHARD_GIT_R2_SECRET_ACCESS_KEY;
  if (![endpoint, bucket, access, secret].every(value => typeof value === "string" && value)) {
    throw new Error("Git pack R2 credentials are unavailable");
  }
  const client = s3Client(endpoint, access, secret);
  const object = key => atObject(client, objectPath(bucket, key));
  return {
    signPart(key, uploadId, partNumber, size) {
      return lfsPresign(endpoint, bucket, access, secret, key, uploadId, partNumber, size, "");
    },
    parts(key, uploadId) {
      return listUploadParts(client, bucket, key, uploadId);
    },
    async copy(from, to, source, timeout) {
      const signal = AbortSignal.timeout(timeout);
      const copySource = objectPath(bucket, from);
      if (source.size > maxPartBytes) {
        return copyMultipart(client, object(to), object(from), copySource, source.size, source.httpEtag, signal, timeout);
      }
      const reply = await s3Request(client, "CopyObject", object(to), {
        method: "PUT",
        headers: { "x-amz-copy-source": copySource, "x-amz-copy-source-if-match": source.httpEtag },
        signal,
        xmlRoot: "CopyObjectResult",
      });
      quotedEtag(field(reply, "ETag"));
      return true;
    },
  };
}
