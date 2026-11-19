import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { createMessage, generateKey, sign } from "openpgp";

export const gpgSettings = "/user/settings/gpg";

export function gpgForm(csrf, fields) {
  const form = new FormData();
  for (const [name, value] of Object.entries({ csrf_token: csrf, ...fields })) form.set(name, value);
  return form;
}

export async function signedCommit(git, worktree, email, { tamper = false } = {}) {
  const date = new Date(Date.now() - 60_000);
  const { privateKey, publicKey } = await generateKey({
    type: "ecc", curve: "curve25519Legacy", userIDs: [{ name: "Signer", email }], date, format: "object",
  });
  const parent = await git("rev-parse", "HEAD");
  const tree = await git("rev-parse", "HEAD^{tree}");
  const timestamp = Math.floor(date.getTime() / 1_000);
  const header = `tree ${tree}\nparent ${parent}\nauthor Contributor <author@example.test> ${timestamp} +0000\n` +
    `committer Signer <${email.toUpperCase()}> ${timestamp} +0000\n`;
  const message = "Signed fixture\n\nCode review · 审查\n";
  const binary = Buffer.from(`${header}\n${message}`);
  const armor = await sign({ message: await createMessage({ binary }), signingKeys: privateKey, detached: true });
  const signature = `gpgsig ${armor.trimEnd().replaceAll("\n", "\n ")}\n`;
  const fixture = join(worktree, ".git", "signed-fixture");
  async function writeCommit(body) {
    await writeFile(fixture, `${header}${signature}\n${body}`);
    return git("hash-object", "-t", "commit", "-w", fixture);
  }
  const signed = await writeCommit(message);
  const tip = await git("commit-tree", tree, "-p", signed, "-m", "Keep the signature on a historical commit");
  await git("update-ref", "refs/heads/gpg-fixture", tip);
  const altered = tamper ? await writeCommit(message.replace("review", "change")) : null;
  if (altered) await git("update-ref", "refs/heads/gpg-tampered", altered);
  return { signed, altered, fingerprint: publicKey.getFingerprint(), publicKey: publicKey.armor(), privateKey: privateKey.armor() };
}
