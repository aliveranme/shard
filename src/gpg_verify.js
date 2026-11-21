import { createMessage, enums, readKeys, readSignature, verify } from "openpgp";

const MAX_PACKETS = 64;
const MAX_SIGNATURES = 32;

async function verifyMatchingUserID(key, email, date) {
  const normalized = email.toLowerCase();
  const candidates = key.users.filter(({ userID }) => userID?.email?.toLowerCase() === normalized);
  for (const candidate of candidates) {
    const query = { email: candidate.userID.email };
    try {
      // Explicitly verify the User ID packet self-certification and revocation
      // for both OpenPGP v4 and v6 keys (v6 getSigningKey skips getPrimaryUser).
      const primary = await key.getPrimaryUser(date, query);
      if (primary?.user?.userID?.email?.toLowerCase() !== normalized) continue;
      if (await primary.user.isRevoked(key.keyPacket, null, date)) continue;
      return query;
    } catch {
      // Try remaining User ID packets with the same email (e.g. if an older UID was revoked).
    }
  }
  return null;
}

export async function shard_gpg_key(armor, email) {
  const begin = "-----BEGIN PGP PUBLIC KEY BLOCK-----";
  if (!armor.startsWith(`${begin}\n`) || !armor.endsWith("\n-----END PGP PUBLIC KEY BLOCK-----") ||
      armor.includes("-----BEGIN PGP ", begin.length)) throw new Error("Expected one public key");
  const keys = await readKeys({ armoredKeys: armor });
  const key = keys[0];
  const packets = key?.toPacketList();
  if (keys.length !== 1 || key.isPrivate() || key.subkeys.length > 8 || key.users.length > 16 ||
      !packets || packets.length > MAX_PACKETS ||
      packets.filterByTag(enums.packet.signature).length > MAX_SIGNATURES ||
      packets.filterByTag(enums.packet.secretKey, enums.packet.secretSubkey).length) {
    throw new Error("Invalid public key certificate");
  }
  const date = new Date();
  const userID = await verifyMatchingUserID(key, email, date);
  if (!userID) return "";
  await key.getSigningKey(null, date, userID);
  // Only index key IDs that actually validate as signing keys.
  const verifiedKeyIDs = [];
  for (const id of key.getKeyIDs()) {
    try {
      await key.getSigningKey(id, date, userID);
      verifiedKeyIDs.push(id.toHex());
    } catch {
      // Skip non-signing, revoked, or expired subkeys.
    }
  }
  if (!verifiedKeyIDs.length) throw new Error("No valid signing key found");
  return JSON.stringify({
    fingerprint: key.getFingerprint(),
    key_ids: [...new Set(verifiedKeyIDs)],
    public_key: key.armor(),
  });
}

export async function shard_gpg_signature(armor) {
  const signature = await readSignature({ armoredSignature: armor });
  if (signature.packets.length !== 1 || signature.packets[0].signatureType !== enums.signature.binary) {
    throw new Error("Expected one binary detached signature");
  }
  const [id] = signature.getSigningKeyIDs();
  return [id.toHex(), signature];
}

export async function shard_git_commit_oid(bytes) {
  const prefix = new TextEncoder().encode(`commit ${bytes.length}\0`);
  const object = new Uint8Array(prefix.length + bytes.length);
  object.set(prefix);
  object.set(bytes, prefix.length);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-1", object));
  return digest.toHex();
}

export async function shard_gpg_verify(binary, signature, armor, email) {
  const [key] = await readKeys({ armoredKeys: armor });
  if (!key) return false;
  const date = new Date();
  const userID = await verifyMatchingUserID(key, email, date);
  if (!userID) return false;
  const [keyID] = signature.getSigningKeyIDs();
  // Check revocation and expiry against the current certificate as well as
  // the message verifier's signing-time checks.
  await key.getSigningKey(keyID, date, userID);
  const result = await verify({ message: await createMessage({ binary }), signature, verificationKeys: key, date });
  return (await result.signatures[0]?.verified) === true;
}
