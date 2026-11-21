use std::fmt;

catalog! {
    NotificationText {
        Notifications => "Notifications", "通知";
        Empty => "You have no notifications yet.", "你还没有收到通知。";
        PageEmpty => "There are no notifications on this page.", "此页没有通知。";
        Unread => "Unread", "未读";
        MarkRead => "Mark as read", "标为已读";
        MarkAllRead => "Mark all as read", "全部标为已读";
        NotFound => "Notification not found.", "未找到通知。";
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationAction {
    Opened,
    Commented,
    Closed,
    Reopened,
    Approved,
    ChangesRequested,
    LineComment,
    MergeVerified,
}

impl super::Locale {
    pub fn notification_activity(
        self,
        action: NotificationAction,
        actor: &str,
    ) -> impl fmt::Display + '_ {
        use NotificationAction::*;

        fmt::from_fn(move |f| match (self, action) {
            (Self::En, Opened) => write!(f, "{actor} opened the discussion"),
            (Self::ZhHans, Opened) => write!(f, "{actor} 发起了讨论"),
            (Self::En, Commented) => write!(f, "{actor} commented"),
            (Self::ZhHans, Commented) => write!(f, "{actor} 发表了评论"),
            (Self::En, Closed) => write!(f, "{actor} closed the discussion"),
            (Self::ZhHans, Closed) => write!(f, "{actor} 关闭了讨论"),
            (Self::En, Reopened) => write!(f, "{actor} reopened the discussion"),
            (Self::ZhHans, Reopened) => write!(f, "{actor} 重新打开了讨论"),
            (Self::En, Approved) => write!(f, "{actor} approved the pull request"),
            (Self::ZhHans, Approved) => write!(f, "{actor} 批准了合并请求"),
            (Self::En, ChangesRequested) => write!(f, "{actor} requested changes"),
            (Self::ZhHans, ChangesRequested) => write!(f, "{actor} 请求了更改"),
            (Self::En, LineComment) => write!(f, "{actor} left an inline comment"),
            (Self::ZhHans, LineComment) => write!(f, "{actor} 发表了行内评论"),
            (Self::En, MergeVerified) => write!(f, "{actor} verified the merge"),
            (Self::ZhHans, MergeVerified) => write!(f, "{actor} 确认了合并"),
        })
    }
}
