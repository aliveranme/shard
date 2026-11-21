use worker::{Env, Error, Result};

const DEFAULT_LFS_BYTES: u64 = 100_000_000;
pub(crate) const R2_SINGLE_PUT_BYTES: u64 = (5 * 1024 - 5) * 1024 * 1024;
const R2_OBJECT_BYTES: u64 = (5 * 1024 - 5) * 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Profile {
    Free,
    Paid,
}

impl Profile {
    pub(crate) fn read(env: &Env) -> Result<Self> {
        let value = env
            .var("SHARD_LIMIT_PROFILE")
            .ok()
            .map(|value| value.to_string());
        match value.as_deref() {
            None | Some("free") => Ok(Self::Free),
            Some("paid") => Ok(Self::Paid),
            Some(_) => Err(Error::RustError(
                "SHARD_LIMIT_PROFILE must be free or paid".into(),
            )),
        }
    }
}

pub(crate) struct ScheduledLimits {
    pub(crate) repo_copies: i64,
    pub(crate) repo_cleanups: i64,
    pub(crate) lfs_copy_objects: u32,
    pub(crate) lfs_reconcile_objects: u32,
    pub(crate) lfs_cleanup_objects: u32,
    pub(crate) push_intents: i64,
    pub(crate) pr_refreshes: i64,
    pub(crate) merges: i64,
    pub(crate) webhook_subscriptions: usize,
    pub(crate) webhook_legacy: usize,
}

impl ScheduledLimits {
    pub(crate) fn read(env: &Env) -> Result<Self> {
        let s3_configured = env.var("SHARD_LFS_R2_ENDPOINT").is_ok();
        Ok(match Profile::read(env)? {
            Profile::Free => Self {
                repo_copies: 2,
                repo_cleanups: 1,
                lfs_copy_objects: 2,
                lfs_reconcile_objects: if s3_configured { 1 } else { 2 },
                lfs_cleanup_objects: 256,
                push_intents: 1,
                pr_refreshes: 1,
                merges: 1,
                webhook_subscriptions: 1,
                webhook_legacy: 1,
            },
            Profile::Paid => Self {
                // A maximal S3 multipart copy uses 1,024 part requests plus setup and completion.
                // Reserve capacity for other jobs in the same invocation, even at the Paid default of 10,000.
                repo_copies: if s3_configured { 3 } else { 12 },
                repo_cleanups: 12,
                lfs_copy_objects: 8,
                lfs_reconcile_objects: if s3_configured { 1 } else { 16 },
                lfs_cleanup_objects: 1_000,
                push_intents: 24,
                pr_refreshes: if s3_configured { 28 } else { 32 },
                merges: 12,
                webhook_subscriptions: 32,
                webhook_legacy: 32,
            },
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LfsMode {
    Proxy,
    Direct,
    Multipart,
}

pub(crate) struct LfsLimits {
    pub(crate) mode: LfsMode,
    pub(crate) max_bytes: u64,
}

impl LfsLimits {
    pub(crate) fn read(env: &Env) -> Result<Self> {
        let configured = env
            .var("SHARD_LFS_UPLOAD_MODE")
            .ok()
            .map(|value| value.to_string());
        let (mode, maximum, default) = match configured.as_deref() {
            None | Some("proxy") => {
                let maximum = match Profile::read(env)? {
                    Profile::Free => DEFAULT_LFS_BYTES,
                    Profile::Paid => R2_SINGLE_PUT_BYTES,
                };
                (LfsMode::Proxy, maximum, DEFAULT_LFS_BYTES)
            }
            Some("r2-direct") => (LfsMode::Direct, R2_SINGLE_PUT_BYTES, R2_SINGLE_PUT_BYTES),
            Some("r2-multipart") => {
                // Above one PUT, verification hashes the R2 stream in the Worker.
                let maximum = match Profile::read(env)? {
                    Profile::Free => R2_SINGLE_PUT_BYTES,
                    Profile::Paid => R2_OBJECT_BYTES,
                };
                (LfsMode::Multipart, maximum, R2_SINGLE_PUT_BYTES)
            }
            Some(_) => {
                return Err(Error::RustError(
                    "SHARD_LFS_UPLOAD_MODE must be proxy, r2-direct or r2-multipart".into(),
                ))
            }
        };
        let max_bytes = match env.var("SHARD_LFS_MAX_BYTES") {
            Ok(value) => value
                .to_string()
                .parse()
                .ok()
                .filter(|value| (1..=maximum).contains(value))
                .ok_or_else(|| {
                    Error::RustError(format!(
                        "SHARD_LFS_MAX_BYTES must be between 1 and {maximum}"
                    ))
                })?,
            Err(_) => default,
        };
        Ok(Self { mode, max_bytes })
    }
}
