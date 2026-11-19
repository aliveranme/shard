import { createMessage, enums, readKeys, readSignature, verify } from "openpgp";

function userIDFor(key, email) {
  const normalized = email.toLowerCase();
  const user = key.users.find(({ userID }) => userID?.email?.toLowerCase() === normalized);
  return user ? { email: user.userID.email } : null;
}

export async function shard_gpg_key(armor, email) {
  const begin = "-----BEGIN PGP PUBLIC KEY BLOCK-----";
  if (!armor.startsWith(`${begin}\n`) || !armor.endsWith("\n-----END PGP PUBLIC KEY BLOCK-----") ||
      armor.includes("-----BEGIN PGP ", begin.length)) throw new Error("Expected one public key");
  const keys = await readKeys({ armoredKeys: armor });
  const key = keys[0];
  if (keys.length !== 1 || key.isPrivate() || key.subkeys.length > 8 || key.users.length > 16 ||
      key.toPacketList().filterByTag(enums.packet.secretKey, enums.packet.secretSubkey).length) {
    throw new Error("Invalid public key certificate");
  }
  const userID = userIDFor(key, email);
  if (!userID) return "";
  await key.getSigningKey(null, new Date(), userID);
  return JSON.stringify({
    fingerprint: key.getFingerprint(),
    key_ids: key.getKeyIDs().map(id => id.toHex()),
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
  const userID = userIDFor(key, email);
  if (!userID) return false;
  const date = new Date();
  const [keyID] = signature.getSigningKeyIDs();
  // Check revocation and expiry against the current certificate as well as
  // the message verifier's signing-time checks.
  await key.getSigningKey(keyID, date, userID);
  const result = await verify({ message: await createMessage({ binary }), signature, verificationKeys: key, date });
  return (await result.signatures[0]?.verified) === true;
}
