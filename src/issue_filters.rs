use crate::db;
use crate::util::{js_int, js_text};
use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::{d1::D1Database, Request, Result};

pub(crate) struct IssueFilters {
    pub(crate) label: String,
    pub(crate) assignee: String,
    pub(crate) milestone: String,
}

impl IssueFilters {
    pub(crate) fn from_request(req: &Request) -> Self {
        let mut label = None;
        let mut assignee = None;
        let mut milestone = None;
        if let Ok(url) = req.url() {
            for (name, value) in url.query_pairs() {
                let field = match name.as_ref() {
                    "label" => &mut label,
                    "assignee" => &mut assignee,
                    "milestone" => &mut milestone,
                    _ => continue,
                };
                field.get_or_insert_with(|| {
                    let trimmed = value.trim();
                    if trimmed.len() == value.len() { value.into_owned() } else { trimmed.to_owned() }
                });
            }
        }
        Self {
            label: label.unwrap_or_default(),
            assignee: assignee.unwrap_or_default(),
            milestone: milestone.unwrap_or_default(),
        }
    }

    pub(crate) fn active(&self) -> bool {
        !self.label.is_empty() || !self.assignee.is_empty() || !self.milestone.is_empty()
    }

    /// Append to a WHERE clause for an issue aliased as `i`.
    pub(crate) fn append(&self, sql: &mut String, args: &mut Vec<JsValue>, query: &str) {
        // Literal matching avoids D1's 50-byte LIKE pattern limit. SQLite's
        // lower() uses the same ASCII case folding as the default LIKE operation.
        let query = query.trim();
        let query = if query.len() > 256 {
            &query[..query.floor_char_boundary(256)]
        } else {
            query
        };
        if !query.is_empty() {
            sql.push_str(
                " AND (instr(lower(i.title), lower(?)) > 0 OR instr(lower(i.body), lower(?)) > 0
                       OR EXISTS (SELECT 1 FROM comments c WHERE c.issue_id = i.id
                                  AND instr(lower(c.body), lower(?)) > 0))",
            );
            let query = js_text(query);
            args.extend([query.clone(), query.clone(), query]);
        }
        for (value, condition) in [
            (&self.label,
             " AND EXISTS (SELECT 1 FROM repo_labels l
                           JOIN issue_labels il ON il.repo_id = l.repo_id AND il.label_id = l.id
                           WHERE l.repo_id = i.repo_id AND l.name = ? AND il.issue_id = i.id)"),
            (&self.assignee,
             " AND i.assignee_id = (SELECT id FROM users WHERE username = ? COLLATE NOCASE)
               AND EXISTS (SELECT 1 FROM repo_assignee_access a
                           WHERE a.repo_id = i.repo_id AND a.user_id = i.assignee_id)"),
            (&self.milestone,
             " AND EXISTS (SELECT 1 FROM issue_milestones im
                           JOIN repo_milestones m ON m.repo_id = im.repo_id AND m.id = im.milestone_id
                           WHERE im.repo_id = i.repo_id AND im.issue_id = i.id AND m.name = ?)"),
        ] {
            if !value.is_empty() {
                sql.push_str(condition);
                args.push(js_text(value));
            }
        }
    }

    pub(crate) async fn repo_counts(
        &self,
        database: &D1Database,
        repo_id: i64,
        kind: &str,
        query: &str,
    ) -> Result<(i64, i64)> {
        #[derive(Deserialize)]
        struct Counts {
            open_count: i64,
            closed_count: i64,
        }
        let mut sql = String::from(
            "SELECT COUNT(CASE WHEN i.state = 'open' THEN 1 END) AS open_count,
                    COUNT(CASE WHEN i.state = 'closed' THEN 1 END) AS closed_count
             FROM issues i WHERE i.repo_id = ? AND i.kind = ?",
        );
        let mut args = vec![js_int(repo_id), js_text(kind)];
        self.append(&mut sql, &mut args, query);
        let counts = db::first::<Counts>(database, &sql, &args).await?;
        Ok(counts.map_or((0, 0), |counts| (counts.open_count, counts.closed_count)))
    }
}
