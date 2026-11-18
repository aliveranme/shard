catalog! {
    LifecycleText {
        RepositoryLifecycle => "Rename or transfer", "重命名或转让";
        Rename => "Rename repository", "重命名仓库";
        RenameHelp => "Existing links and Git remotes keep working under the repository’s current permissions.", "原有链接和 Git 远程地址仍可使用，并遵循仓库当前的权限。";
        Transfer => "Transfer repository", "转让仓库";
        TransferHelp => "The recipient must accept before ownership changes. They receive the repository and its issues and pull requests. The old path stays linked to the repository.", "接收方接受后，仓库的所有权才会变更。仓库、议题和拉取请求将一并转让，原有路径仍可访问该仓库。";
        Recipient => "Recipient username", "接收方用户名";
        RequestTransfer => "Request transfer", "发起转让";
        CancelTransfer => "Cancel transfer", "取消转让";
        TransferPending => "Waiting for this recipient to accept:", "正在等待以下用户接受转让：";
        Transfers => "Repository transfers", "仓库转让";
        IncomingTransfers => "Incoming transfers", "收到的转让";
        TransfersEmpty => "You have no pending incoming transfers.", "你没有待处理的转让请求。";
        Accept => "Accept transfer", "接受转让";
        Decline => "Decline transfer", "拒绝转让";
        Declined => "Transfer declined.", "已拒绝转让。";
        UnknownRecipient => "Choose another account on this site as the recipient.", "请选择此站点的另一个账号作为接收方。";
        NameTaken => "This name is already in use or reserved by an existing path or a pending copy.", "该名称已被使用，或被现有路径或待完成的仓库副本保留。";
        TransferNameTaken => "This name is reserved in the recipient’s namespace. Free the destination name or decline the transfer.", "接收方名下的此名称已被保留。请释放目标名称或拒绝转让。";
        PendingTransferBlocksRename => "Cancel the pending transfer before renaming this repository.", "请先取消待处理的转让，再重命名此仓库。";
        RequestChanged => "The repository or transfer request changed. Review the current state and try again.", "仓库或转让请求已变更。请查看当前状态后重试。";
        TransferNotFound => "Transfer request not found.", "未找到转让请求。";
        Fork => "Fork repository", "派生仓库";
        ForkHelp => "Copy the Git history, branches, and tags into a new repository under your account.", "将 Git 历史、分支和标签复制到你账号下的新仓库。";
        ForkSource => "Source repository", "源仓库";
        PrivateFork => "This fork of a private repository must stay private.", "从私有仓库派生的副本必须保持私有。";
        Import => "Import repository", "导入仓库";
        ImportHelp => "Use the final HTTPS Git URL of a public repository. The source must be reachable without signing in.", "请输入公开仓库的最终 HTTPS Git 地址。源仓库须无需登录即可访问。";
        InvalidName => "Start the repository name with a letter or digit and use only letters, digits, dots, dashes, or underscores.", "仓库名称必须以字母或数字开头，并且只能包含字母、数字、点、连字符或下划线。";
        SourceUrl => "Public HTTPS Git URL", "公开的 HTTPS Git 地址";
        InvalidSource => "Enter a public HTTPS Git URL with a repository path and without credentials, query parameters, or a fragment.", "请输入含仓库路径的公开 HTTPS Git 地址。地址不能含凭据、查询参数或片段。";
        Copies => "Repository copies", "仓库副本";
        CopiesEmpty => "You have no pending or failed copies.", "你没有待完成或失败的复制任务。";
        CopyStatus => "Repository copy status", "仓库复制状态";
        CopyPending => "The repository is being prepared. Check the status again in a moment.", "仓库正在准备中，请稍后再次查看状态。";
        CopyCleanup => "This copy cannot be published. Storage cleanup is still pending.", "此副本无法发布，仍在等待清理存储。";
        CopyFailed => "The copy could not be completed. You can reuse the destination name.", "未能完成复制。你可以重新使用目标仓库名。";
        CopyUnknown => "Git storage could not confirm the copy’s status. Check again later.", "Git 存储暂时无法确认复制状态，请稍后重试。";
        CopyNotFound => "Repository copy not found.", "未找到仓库复制任务。";
        CheckCopy => "Check status", "检查状态";
        BackToRepositorySettings => "Back to repository settings", "返回仓库设置";
        BackToAccountSettings => "Back to account settings", "返回账号设置";
    }
}

impl super::Locale {
    pub fn transfer_destination(self, recipient: &str, name: &str) -> String {
        match self {
            Self::En => format!("Accepting this transfer moves the repository to {recipient}/{name}."),
            Self::ZhHans => format!("接受转让后，仓库将移至 {recipient}/{name}。"),
        }
    }
}
