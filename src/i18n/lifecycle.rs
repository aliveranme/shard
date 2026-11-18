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
        ImportHelp => "Enter the final HTTPS Git URL. Use source credentials when Git requires sign-in.", "请输入最终的 HTTPS Git 地址。如果 Git 要求登录，请填写源仓库凭据。";
        InvalidName => "Start the repository name with a letter or digit and use only letters, digits, dots, dashes, or underscores.", "仓库名称必须以字母或数字开头，并且只能包含字母、数字、点、连字符或下划线。";
        SourceUrl => "HTTPS Git URL", "HTTPS Git 地址";
        InvalidSource => "Enter an HTTPS Git URL on a public hostname with a repository path. Leave credentials, query parameters, and fragments out of the URL.", "请输入使用公网主机名并含仓库路径的 HTTPS Git 地址。请勿在地址中填写凭据、查询参数或片段。";
        ImportCredentialHelp => "Optional. Use a username and password or an access token with the Bearer scheme. Authenticated imports use native Git storage and start as private repositories.", "选填。使用用户名和密码，或使用 Bearer 方式的访问令牌。需要认证的导入使用原生 Git 存储，仓库初始为私有。";
        SourceUsername => "Source username", "源仓库用户名";
        SourcePassword => "Source password or token", "源仓库密码或令牌";
        SourceToken => "Source Bearer token", "源仓库 Bearer 令牌";
        InvalidImportCredential => "Enter both a valid username and password, or one valid Bearer token. Authenticated imports require HTTPS port 443.", "请输入有效的用户名和密码，或单独填写有效的 Bearer 令牌。需要认证的导入必须使用 HTTPS 的 443 端口。";
        ImportNativeRequired => "Authenticated imports require a configured native Git origin.", "需要认证的导入要求已配置原生 Git 源站。";
        Copies => "Repository copies", "仓库副本";
        CopiesEmpty => "You have no pending or failed copies.", "你没有待完成或失败的复制任务。";
        CopyStatus => "Repository copy status", "仓库复制状态";
        CopyPending => "The repository is being prepared in the background. You can return here to check its status.", "仓库正在后台准备，你可以稍后返回此处查看状态。";
        CopyCleanup => "This copy cannot be published. Storage cleanup is still pending.", "此副本无法发布，仍在等待清理存储。";
        CopyFailed => "The copy could not be completed. You can reuse the destination name.", "未能完成复制。你可以重新使用目标仓库名。";
        CopyUnknown => "Git storage could not confirm the copy’s status. Check again later.", "Git 存储暂时无法确认复制状态，请稍后重试。";
        CopyNotFound => "Repository copy not found.", "未找到仓库复制任务。";
        CheckCopy => "Check status", "检查状态";
        BackToRepositorySettings => "Back to repository settings", "返回仓库设置";
        BackToAccountSettings => "Back to account settings", "返回账号设置";
        MetadataImport => "Import repository metadata", "导入仓库元数据";
        MetadataHelp => "Import issues, pull requests, and releases into this repository from a JSON bundle. Copy Git history and tags first.", "从 JSON 数据包向此仓库导入议题、拉取请求和发行版本。请先复制 Git 历史和标签。";
        MetadataBundle => "JSON bundle", "JSON 数据包";
        MetadataFormat => "Bundle format", "数据包格式";
        MetadataLimits => "Each version 1 bundle accepts up to 256 KiB, 100 total threads and releases, and 32 distinct Git commit IDs. Split larger archives across submissions using the same source URL.", "每个版本为 1 的数据包最多为 256 KiB，议题、拉取请求和发行版本合计最多 100 条，且最多引用 32 个不同的 Git 提交 ID。较大的存档可分批提交，每批使用相同的源地址。";
        MetadataThreadFields => "Threads share one numbering sequence. Use open or closed for issues. For pull requests include pull and use open, closed, or merged. Body defaults to empty. All dates use Unix seconds. Closed and merged dates are optional.", "议题和拉取请求共用一套编号。议题状态可为 open 或 closed。拉取请求应包含 pull，状态可为 open、closed 或 merged。正文默认留空。日期使用 Unix 秒数，关闭和合并时间可省略。";
        MetadataPullFields => "A pull request snapshot is optional. Use exact 40-character lowercase Git commit IDs and include the merge base if known. A comparison is available when the required commits exist in this repository. For a head from another repository, include head_source_url. Merged pull requests accept optional merged_at and merge_commit fields.", "拉取请求的比较快照可省略。请填写确切的 40 位小写 Git 提交 ID，并在已知时填写合并基点。只有此仓库中存在所需提交时才能查看比较。若源分支来自其他仓库，请填写 head_source_url。已合并的拉取请求可填写 merged_at 和 merge_commit。";
        MetadataReleaseFields => "A release must match an existing Git tag and commit. Omit published_at for a draft. Notes default to empty. Titles accept up to 200 characters, and Markdown bodies and notes up to 64 KiB each.", "发行版本必须匹配现有 Git 标签和提交。草稿可省略 published_at，说明默认留空。标题最多 200 个字符，Markdown 正文和发行说明各不超过 64 KiB。";
        MetadataSourceFields => "Use the same final HTTPS repository URL in every batch. Set private to true for a private source and use a private destination. Existing issue numbers and release tags must be free.", "每批数据请使用同一个最终 HTTPS 仓库地址。源仓库为私有时，请将 private 设为 true，并使用私有目标仓库。议题和拉取请求编号及发行标签必须尚未使用。";
        MetadataHistory => "The import preserves thread numbers, content, timestamps, and state. Importing historical threads leaves local author accounts unset and sends no notifications. Comments, reviews, and release assets are outside this bundle format.", "导入会保留议题和拉取请求的编号、内容、时间和状态。历史记录不会关联本地作者账号，也不会发送通知。此数据包格式不包含评论、审查或发行附件。";
        MetadataInvalid => "The bundle has invalid or missing fields. Review the format and submit it again.", "数据包包含无效字段或缺少必填字段。请检查格式后重新提交。";
        MetadataTooLarge => "The metadata bundle exceeds an import limit. Split the archive and submit smaller bundles.", "元数据包超出导入限制。请拆分存档后分批提交。";
        MetadataPrivate => "Import metadata from a private source into a private repository.", "请将私有源仓库的元数据导入私有仓库。";
        MetadataSourceMismatch => "The bundle source differs from this repository’s registered metadata source.", "数据包的源地址与此仓库已记录的元数据来源不同。";
        MetadataConflict => "A thread number or release tag is already in use. The bundle was rolled back.", "已有议题或拉取请求使用包内编号，或已有发行版本使用包内标签。数据包已回滚。";
        MetadataTagMismatch => "A release tag is missing or points to a different commit. Check the destination’s Git tags and try again.", "发行标签缺失或指向不同的提交。请检查目标仓库的 Git 标签后重试。";
        MetadataUnavailable => "The import service is temporarily unavailable. Try again later.", "元数据导入暂不可用，请稍后重试。";
        MetadataSource => "Registered metadata source", "已记录的元数据来源";
    }
}

impl super::Locale {
    pub fn transfer_destination(self, recipient: &str, name: &str) -> String {
        match self {
            Self::En => format!("Accepting this transfer moves the repository to {recipient}/{name}."),
            Self::ZhHans => format!("接受转让后，仓库将移至 {recipient}/{name}。"),
        }
    }

    pub fn metadata_imported(self, issues: usize, pulls: usize, releases: usize, without_comparison: usize) -> String {
        match self {
            Self::En => format!("Imported {issues} issues, {pulls} pull requests, and {releases} releases. Pull requests without a comparison: {without_comparison}."),
            Self::ZhHans => format!("已导入 {issues} 个议题、{pulls} 个拉取请求和 {releases} 个发行版本。无法查看比较的拉取请求有 {without_comparison} 个。"),
        }
    }
}
