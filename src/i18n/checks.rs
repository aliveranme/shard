catalog! {
    CheckText {
        Title => "Commit checks", "提交检查";
        Empty => "No checks recorded for this commit.", "此提交暂无检查结果。";
        Omitted => "Additional checks are not shown.", "还有其他检查结果未显示。";
        Pending => "Pending", "等待中";
        Success => "Passed", "通过";
        Failure => "Failed", "失败";
        Error => "Error", "出错";
        CredentialsRequired => "A valid bearer access token is required.", "需要有效的 Bearer 访问令牌。";
        WriteRequired => "A write-scoped access token and repository write permission are required.", "需要具有写入范围的访问令牌及仓库写入权限。";
        JsonRequired => "Send check results as application/json.", "请使用 application/json 提交检查结果。";
        InvalidPayload => "Invalid commit check payload.", "提交检查数据无效。";
        PayloadTooLarge => "The check payload exceeds 8 KiB.", "检查数据超出 8 KiB。";
    }
}
