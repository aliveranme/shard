use crate::git_wire::{RefUpdate, ZERO_OID};
use crate::webhooks;
use worker::Env;

pub(crate) async fn record(env: &Env, repo_id: i64, actor: Option<i64>, updates: &[RefUpdate]) {
    let mut accepted = Vec::new();
    let mut branches = Vec::new();
    for update in updates {
        if update.before == update.after {
            continue;
        }
        if update.name.starts_with("refs/heads/") {
            branches.push(update.name.clone());
        } else if !update.name.starts_with("refs/tags/") {
            continue;
        }
        accepted.push(webhooks::RefUpdate {
            name: &update.name,
            before: (update.before != ZERO_OID).then_some(update.before.as_str()),
            after: (update.after != ZERO_OID).then_some(update.after.as_str()),
        });
    }
    if !accepted.is_empty() && webhooks::record_refs(env, repo_id, actor, &accepted).await.is_err() {
        worker::console_log!("event=git_ref_delivery_record_failed repo_id={repo_id}");
    }
    if !branches.is_empty() && crate::pull_review::enqueue_refs(env, repo_id, &branches).await.is_err() {
        worker::console_log!("event=git_ref_comparison_enqueue_failed repo_id={repo_id}");
    }
}
