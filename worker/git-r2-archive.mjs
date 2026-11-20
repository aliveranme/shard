const OID = /^[a-f0-9]{40}$/;
const BLOCK = 512;
const END = 2 * BLOCK;
const MAX_BYTES = 16 * 1024 * 1024;
const MAX_ENTRIES = 2_000;
const MAX_COMPONENTS = 16;
const PREFIX = 155;
const NAME = 100;
const UTF8 = new TextEncoder();

export class ArchiveLimitError extends Error {}
export class ArchiveUnsupportedError extends Error {}

function octal(header, offset, length, value) {
  header[offset + length - 1] = 0;
  for (let index = offset + length - 2; index >= offset; index--) {
    header[index] = 48 + (value & 7);
    value >>= 3;
  }
  if (value !== 0) throw new ArchiveLimitError("Archive field exceeds USTAR");
}

function align(length) {
  return Math.ceil(length / BLOCK) * BLOCK;
}

function ustarPath(path) {
  if (UTF8.encode(path).length <= NAME) return ["", path];
  for (let at = path.length - 1; at >= 0; at--) {
    if (path[at] !== "/") continue;
    const prefix = path.slice(0, at);
    const name = path.slice(at + 1);
    if (UTF8.encode(prefix).length <= PREFIX && UTF8.encode(name).length <= NAME) {
      return [prefix, name];
    }
  }
  return null;
}

function safeComponent(name) {
  if (!name || name.endsWith(".") || name.endsWith(" ") || name.toLowerCase() === ".git"
      || [...name].some((ch) => ch.charCodeAt(0) < 32 || ch === "\u{fffd}")
      || /[\/\\:<>"|?*]/.test(name)) return false;
  const stem = (name.split(".")[0] ?? "").replace(/ +$/, "");
  if (["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].some((device) => stem.toLowerCase() === device.toLowerCase())) return false;
  if (/^(com|lpt)[1-9¹²³]$/i.test(stem)) return false;
  return true;
}

function header(prefix, name, mode, kind, size) {
  const block = new Uint8Array(BLOCK);
  const nameBytes = UTF8.encode(name);
  const prefixBytes = UTF8.encode(prefix);
  if (nameBytes.length > NAME || prefixBytes.length > PREFIX) throw new ArchiveUnsupportedError("Archive path is too long");
  block.set(nameBytes, 0);
  octal(block, 100, 8, mode);
  octal(block, 108, 8, 0);
  octal(block, 116, 8, 0);
  octal(block, 124, 12, size);
  octal(block, 136, 12, 0);
  block.fill(32, 148, 156);
  block[156] = kind;
  block.set(UTF8.encode("ustar\0" + "00"), 257);
  block.set(prefixBytes, 345);
  let checksum = 0;
  for (const byte of block) checksum += byte;
  octal(block, 148, 7, checksum);
  block[155] = 32;
  return block;
}

/** Build the same bounded USTAR source archive as the Worker code generator. */
export async function buildSourceTar(commit, readObject) {
  if (!commit || !OID.test(commit.tree ?? "") || !OID.test(commit.hash ?? "")) {
    throw new ArchiveUnsupportedError("Commit has no valid tree");
  }
  const chunks = [];
  let total = 0;
  const append = (path, mode, kind, contents) => {
    if (total + BLOCK + align(contents.length) + END > MAX_BYTES) throw new ArchiveLimitError("Archive is too large");
    const [prefix, name] = ustarPath(path) ?? (() => { throw new ArchiveUnsupportedError("Archive path is too long"); })();
    chunks.push(header(prefix, name, mode, kind, contents.length));
    chunks.push(contents);
    const padding = align(contents.length) - contents.length;
    if (padding) chunks.push(new Uint8Array(padding));
    total += BLOCK + align(contents.length);
  };
  const root = `source-${commit.hash}`;
  const nodes = [{ path: root, mode: 0o755, kind: 53 }];
  let remaining = MAX_ENTRIES;
  const walk = async (tree, path, depth) => {
    const object = await readObject(tree);
    if (!object || object.type !== 2) throw new ArchiveUnsupportedError("Missing Git tree");
    const entries = object.data ? parseTreeEntries(object.data) : [];
    remaining -= entries.length;
    if (remaining < 0) throw new ArchiveLimitError("Archive has too many entries");
    if (depth >= MAX_COMPONENTS && entries.length > 0) throw new ArchiveLimitError("Archive is too deep");
    if (new Set(entries.map((entry) => entry.name)).size !== entries.length) {
      throw new ArchiveUnsupportedError("Duplicate tree entry");
    }
    entries.sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0));
    for (const entry of entries) {
      if (!safeComponent(entry.name)) throw new ArchiveUnsupportedError("Unsupported archive entry");
      const child = `${path}/${entry.name}`;
      if (entry.mode === "040000") {
        nodes.push({ path: child, mode: 0o755, kind: 53 });
        await walk(entry.hash, child, depth + 1);
      } else if (entry.mode === "100644" || entry.mode === "100755") {
        nodes.push({ path: child, mode: entry.mode === "100755" ? 0o755 : 0o644, kind: 48, hash: entry.hash });
      } else {
        throw new ArchiveUnsupportedError("Unsupported Git mode");
      }
    }
  };
  await walk(commit.tree, root, 0);
  const files = nodes.filter((node) => node.hash);
  for (const node of files) {
    const blob = await readObject(node.hash);
    if (!blob || blob.type !== 3) throw new ArchiveUnsupportedError("Missing Git blob");
    node.data = blob.data ?? new Uint8Array();
  }
  for (const node of nodes) append(node.path, node.mode, node.kind, node.data ?? new Uint8Array());
  chunks.push(new Uint8Array(END));
  total += END;
  const result = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}

function parseTreeEntries(data) {
  const entries = [];
  let offset = 0;
  while (offset < data.length) {
    const space = data.indexOf(32, offset);
    const nul = data.indexOf(0, space + 1);
    if (space < 0 || nul < 0 || nul + 21 > data.length) throw new ArchiveUnsupportedError("Invalid Git tree");
    const mode = new TextDecoder().decode(data.subarray(offset, space));
    const name = new TextDecoder("utf-8", { fatal: true }).decode(data.subarray(space + 1, nul));
    const hash = [...data.subarray(nul + 1, nul + 21)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    entries.push({ mode: mode === "40000" ? "040000" : mode, name, hash });
    offset = nul + 21;
  }
  return entries;
}