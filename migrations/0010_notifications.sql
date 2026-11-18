-- Store stable thread IDs and event facts. Titles, routes and access come from live rows.
CREATE TABLE notification_events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
  actor_id    INTEGER REFERENCES users(id) ON DELETE SET NULL,
  action      TEXT NOT NULL CHECK (action IN ('opened', 'commented', 'closed', 'reopened',
                                              'approved', 'changes_requested', 'line_comment', 'merge_verified')),
  created_at  INTEGER NOT NULL
);
CREATE INDEX notification_events_issue ON notification_events(issue_id);
CREATE INDEX notification_events_actor ON notification_events(actor_id);

CREATE TABLE notifications (
  recipient_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  event_id     INTEGER NOT NULL REFERENCES notification_events(id) ON DELETE CASCADE,
  read_at      INTEGER,
  PRIMARY KEY (recipient_id, event_id)
) WITHOUT ROWID;
CREATE INDEX notifications_event ON notifications(event_id);
CREATE INDEX notifications_unread ON notifications(recipient_id, event_id) WHERE read_at IS NULL;

-- Subscribe the owner, author, current assignee and anyone who has commented or
-- reviewed. Select each recipient once and respect private access at write time.
CREATE TRIGGER notification_recipients AFTER INSERT ON notification_events
BEGIN
  INSERT INTO notifications (recipient_id, event_id)
  SELECT u.id, NEW.id
    FROM issues i JOIN repos r ON r.id = i.repo_id
    JOIN users u ON u.id IN (
      SELECT r.owner_id
      UNION ALL SELECT i.author_id
      UNION ALL SELECT i.assignee_id
      UNION ALL SELECT c.author_id FROM comments c WHERE c.issue_id = NEW.issue_id
      UNION ALL SELECT p.actor_id FROM pull_reviews p WHERE p.issue_id = NEW.issue_id
    )
   WHERE i.id = NEW.issue_id AND u.id IS NOT NEW.actor_id
     AND (r.visibility = 'public' OR r.owner_id = u.id OR u.is_admin = 1
          OR EXISTS (SELECT 1 FROM repo_collaborators c WHERE c.repo_id = r.id AND c.user_id = u.id));
END;

-- Capture events at source so a failed issue/PR write rolls back its notifications.
CREATE TRIGGER notification_issue_opened AFTER INSERT ON issues
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.id, NEW.author_id, 'opened', NEW.created_at);
END;

CREATE TRIGGER notification_comment_added AFTER INSERT ON comments
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.issue_id, NEW.author_id, 'commented', NEW.created_at);
END;

CREATE TRIGGER notification_state_changed AFTER INSERT ON issue_events
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.issue_id, NEW.actor_id, NEW.event, NEW.created_at);
END;

-- Merge verification records its own review event. Its accompanying issue UPDATE
-- is deliberately not a second close notification.
CREATE TRIGGER notification_pull_reviewed AFTER INSERT ON pull_reviews
BEGIN
  INSERT INTO notification_events (issue_id, actor_id, action, created_at)
  VALUES (NEW.issue_id, NEW.actor_id, NEW.decision, NEW.created_at);
END;
