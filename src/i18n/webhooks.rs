catalog! {
    WebhookText {
        Webhooks => "Webhooks", "Webhook";
        Add => "Add webhook", "添加 Webhook";
        Subscriptions => "Subscriptions", "订阅";
        Empty => "This repository has no webhooks.", "此仓库还没有 Webhook。";
        Endpoint => "Endpoint URL", "接收地址";
        EndpointHelp => "Use an HTTPS URL on a public hostname.", "请使用带有公网主机名的 HTTPS 地址。";
        Label => "Name (optional)", "名称（选填）";
        Secret => "Signing secret", "签名密钥";
        SecretHelp => "Use a randomly generated secret and configure the same value at your receiver.", "请使用随机生成的密钥，并在接收端配置相同的值。";
        EventHelp => "Receives issue, pull request, branch push, and tag events from this repository.", "接收此仓库的议题、合并请求、分支推送和标签事件。";
        Active => "Active", "已启用";
        Disabled => "Disabled", "已停用";
        Disable => "Disable", "停用";
        Remove => "Delete…", "删除…";
        Delete => "Delete webhook", "删除 Webhook";
        DeleteHelp => "Deleting this webhook cancels queued deliveries and removes its delivery history.", "删除此 Webhook 会取消排队中的投递并清除投递记录。";
        Recent => "Recent deliveries", "最近投递";
        NoDeliveries => "No deliveries yet.", "尚无投递记录。";
        Delivery => "Delivery", "投递编号";
        Attempts => "Attempts", "尝试次数";
        Push => "Branch push", "分支推送";
        Tag => "Tag change", "标签变更";
        Issue => "Issue", "议题";
        PullRequest => "Pull request", "合并请求";
        Pending => "Queued", "排队中";
        Sending => "Sending", "投递中";
        Sent => "Delivered", "已送达";
        Failed => "Failed", "失败";
        Canceled => "Canceled", "已取消";
        InvalidEndpoint => "Enter a public HTTPS endpoint URL without credentials or a fragment.", "请输入不含凭证或片段的公网 HTTPS 接收地址。";
        InvalidLabel => "Use at most 100 characters for the webhook name.", "Webhook 名称不得超过 100 个字符。";
        InvalidSecret => "Enter a signing secret of up to 1024 bytes without control characters.", "请输入不含控制字符且不超过 1024 字节的签名密钥。";
        KeyUnavailable => "An instance administrator must configure the webhook encryption key before you can add webhooks.", "实例管理员配置 Webhook 加密密钥后，才可添加 Webhook。";
        InvalidAction => "Choose a valid webhook action.", "请选择有效的 Webhook 操作。";
        SubscriptionNotFound => "Webhook not found.", "未找到 Webhook。";
        CreateConflict => "An active webhook already uses this endpoint, or the repository has reached its 10-webhook limit.", "该地址已有启用的 Webhook，或仓库已达到 10 个 Webhook 的上限。";
    }
}
