import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

const cloudBuild = process.env.WORKERS_CI === "1";

// Workers Builds already ran npm run build before Wrangler's custom build hook.
if (cloudBuild && process.env.WRANGLER_COMMAND && existsSync("build/index.js")) {
  process.exit(0);
}

const env = { ...process.env };
function run(command, args, options) {
  const result = spawnSync(command, args, { env, stdio: "inherit", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
  return result.stdout;
}

if (cloudBuild) {
  const cargoBin = join(env.CARGO_HOME || join(homedir(), ".cargo"), "bin");
  env.PATH = [cargoBin, env.PATH].filter(Boolean).join(delimiter);
  const rustup = spawnSync("rustup", ["toolchain", "list"], { env, stdio: "ignore" });
  if (rustup.error && rustup.error.code !== "ENOENT") throw rustup.error;
  if (rustup.error?.code === "ENOENT") {
    const installer = run("curl", ["--proto", "=https", "--tlsv1.2", "-fsSL", "https://sh.rustup.rs"],
      { stdio: ["ignore", "pipe", "inherit"] });
    run("sh", ["-s", "--", "-y", "--default-toolchain", "none", "--no-modify-path"],
      { input: installer, stdio: ["pipe", "inherit", "inherit"] });
  }
  // The rustup proxies use the channel and Wasm target in rust-toolchain.toml.
  run("cargo", ["install", "worker-build", "--version", "=0.9.0", "--locked"]);
}

run("worker-build", ["--release"]);
