use std::fs;

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn emit_asset(env_name: &str, path: &str) {
    println!("cargo:rerun-if-changed={path}");
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("failed to read {path}: {err}"));
    let hash = fnv1a_64(&bytes);
    println!("cargo:rustc-env={env_name}={hash:016x}");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    emit_asset("SHARD_THEME_CSS_V", "static/css/theme.css");
    emit_asset("SHARD_SHARD_CSS_V", "static/css/shard.css");
    emit_asset("SHARD_ICONS_SVG_V", "static/img/icons.svg");
    emit_asset("SHARD_LOGO_SVG_V", "static/img/logo.svg");
}
