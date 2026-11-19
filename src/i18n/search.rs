catalog! {
    SearchText {
        RepositorySearch => "Search repository", "搜索仓库";
        Query => "Search text", "搜索文本";
        SearchIn => "Search in", "搜索范围";
        Paths => "File paths", "文件路径";
        Code => "File contents", "文件内容";
        Commits => "Commit messages", "提交消息";
        Reference => "Branch, tag or commit", "分支、标签或提交";
        PathCoverage => "Search is case-sensitive and matches full file paths at the selected Git revision.", "区分大小写，在所选 Git 版本中搜索完整文件路径。";
        CodeCoverage => "Search is case-sensitive and shows the first matching line in each UTF-8 text file up to 64 KiB.", "区分大小写，显示每个不超过 64 KiB 的 UTF-8 文本文件中首个匹配的行。";
        SearchedCommit => "Searched commit", "搜索所用的提交";
        PartialResults => "Partial results", "部分结果";
        ScanLimit => "The repository scan reached its limit. Some paths or files were skipped.", "已达到仓库扫描上限。部分路径或文件已跳过。";
        HistoryLimit => "Older commits were outside the search window.", "更早的提交不在搜索范围内。";
        LargeFiles => "Files larger than 64 KiB were skipped.", "已跳过大于 64 KiB 的文件。";
        LongMessages => "Only the first 64 KiB of longer commit messages were searched.", "较长的提交消息仅搜索前 64 KiB。";
        ResultLimit => "Showing the first 40 matches.", "仅显示前 40 条匹配结果。";
        ReadFailed => "Some Git objects could not be read.", "无法读取部分 Git 对象。";
        NoResults => "No matches found.", "未找到匹配项。";
        NoPartialResults => "No matches found in the portion searched.", "已搜索的范围内没有匹配项。";
        RefNotFound => "This branch, tag or commit could not be resolved.", "找不到此分支、标签或提交。";
        InvalidRef => "Enter a valid branch, tag or commit ID.", "请输入有效的分支、标签或提交 ID。";
        InvalidQuery => "Enter a single line of up to 256 bytes.", "请输入不超过 256 字节的单行文本。";
        AssigneeUsername => "Assignee username", "负责人的用户名";
        Line => "Line", "行号";
    }
}

impl super::Locale {
    pub fn commit_search_coverage(self, commits: &usize) -> String {
        match self {
            Self::En => format!("Search is case-sensitive and covers commit messages and ID prefixes in the latest {commits} first-parent commits."),
            Self::ZhHans => format!("区分大小写，在第一父提交链最近的 {commits} 次提交中搜索提交消息和提交 ID 前缀。"),
        }
    }
}
