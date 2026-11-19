catalog! {
    GpgText {
        Keys => "GPG keys", "GPG 密钥";
        AddKey => "Add GPG key", "添加 GPG 密钥";
        Name => "Key name", "密钥名称";
        PublicKey => "Armored public key", "ASCII 格式公钥";
        PublicKeyHelp => "Export the public key with gpg --armor --export. Use a key with your account email.", "用 gpg --armor --export 导出公钥。公钥需包含你的账号邮箱。";
        SavedKeys => "Your signing keys", "你的签名密钥";
        Empty => "You have no GPG keys", "你还没有登记 GPG 密钥";
        Saved => "GPG key saved", "GPG 密钥已保存";
        Removed => "GPG key removed", "GPG 密钥已移除";
        Remove => "Remove key", "移除密钥";
        RemoveHelp => "Removing this key stops Shard from verifying commits with it.", "移除此密钥后，Shard 将无法用它验证提交。";
        InvalidForm => "The GPG key form could not be read", "无法读取 GPG 密钥表单";
        InvalidName => "Enter a key name up to 100 characters", "请输入不超过 100 个字符的密钥名称";
        InvalidKey => "Enter a single, active OpenPGP public signing key", "请输入一把有效的 OpenPGP 签名公钥";
        KeyTooLarge => "Use a public key up to 32 KiB", "请使用不超过 32 KiB 的公钥";
        EmailMismatch => "The key must include your current account email", "公钥必须包含当前账号邮箱";
        KeyLimit => "You can keep up to eight GPG keys. Refresh the page if your account email changed.", "每个账号最多可保存八把 GPG 密钥。如果邮箱已变更，请刷新页面。";
        Unsigned => "Unsigned commit", "提交未签名";
        Unsupported => "Unsupported signature format", "签名格式暂不支持";
        MissingKey => "No registered GPG key for this signature", "未登记此签名对应的 GPG 公钥";
        InvalidSignature => "GPG verification failed", "GPG 签名验证失败";
        Unavailable => "Signature verification unavailable", "暂时无法验证签名";
        ValidSignature => "Valid GPG signature", "GPG 签名有效";
        VerifiedEmail => "Committer email verified on Shard", "提交者邮箱已在 Shard 验证";
    }
}
