use worker::{Env, Error, Result};

const DEFAULT_LFS_BYTES: u64 = 100_000_000;
const R2_SINGLE_PUT_BYTES: u64 = (5 * 1024 - 5) * 1024 * 1024;

#[derive(Clone, Copy)]
enum Profile {
    Free,
    Paid,
}

impl Profile {
    fn read(env: &Env) -> Result<Self> {
        let value = env.var("SHARD_LIMIT_PROFILE").ok().map(|value| value.to_string());
        match value.as_deref() {
            None | Some("free") => Ok(Self::Free),
            Some("paid") => Ok(Self::Paid),
            Some(_) => Err(Error::RustError("SHARD_LIMIT_PROFILE must be free or paid".into())),
        }
    }
}

pub(crate) struct ScheduledLimits {
    pub(crate) repo_copies: i64,
    pub(crate) repo_cleanups: i64,
    pub(crate) lfs_copy_objects: u32,
    pub(crate) lfs_cleanup_objects: u32,
    pub(crate) push_intents: i64,
    pub(crate) pr_refreshes: i64,
    pub(crate) merges: i64,
    pub(crate) webhook_subscriptions: usize,
    pub(crate) webhook_legacy: usize,
}

impl ScheduledLimits {
    pub(crate) fn read(env: &Env) -> Result<Self> {
        Ok(match Profile::read(env)? {
            Profile::Free => Self {
                repo_copies: 2,
                repo_cleanups: 1,
                lfs_copy_objects: 2,
                lfs_cleanup_objects: 64,
                push_intents: 1,
                pr_refreshes: 1,
                merges: 1,
                webhook_subscriptions: 1,
                webhook_legacy: 1,
            },
            Profile::Paid => Self {
                repo_copies: 8,
                repo_cleanups: 8,
                lfs_copy_objects: 8,
                lfs_cleanup_objects: 256,
                push_intents: 12,
                pr_refreshes: 24,
                merges: 12,
                webhook_subscriptions: 16,
                webhook_legacy: 16,
            },
        })
    }
}

pub(crate) fn lfs_object_bytes(env: &Env) -> Result<u64> {
    let maximum = match Profile::read(env)? {
        Profile::Free => DEFAULT_LFS_BYTES,
        Profile::Paid => R2_SINGLE_PUT_BYTES,
    };
    let Ok(configured) = env.var("SHARD_LFS_MAX_BYTES") else {
        return Ok(DEFAULT_LFS_BYTES);
    };
    configured.to_string().parse().ok().filter(|value| (1..=maximum).contains(value))
        .ok_or_else(|| Error::RustError(format!("SHARD_LFS_MAX_BYTES must be between 1 and {maximum}")))
}
