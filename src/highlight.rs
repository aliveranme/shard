//! Small server-side syntax highlighter for repository text previews.
//!
//! The Worker renders HTML without scripts, so highlighting happens here. The
//! scanner preserves the source text exactly; only spans are inserted.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    JavaScript,
    Python,
    Go,
    Java,
    C,
    Css,
    Html,
    Shell,
    Sql,
    Yaml,
    Toml,
    Json,
    Dockerfile,
    Makefile,
}

pub(crate) fn render_lines(path: &str, source: &str) -> (Vec<String>, bool) {
    let (rendered, highlighted) = match render(path, source) {
        Some(html) => (html, true),
        None => {
            let mut escaped = String::with_capacity(source.len());
            escape_into(&mut escaped, source);
            (escaped, false)
        }
    };
    let mut lines: Vec<String> = rendered
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect();
    if source.ends_with('\n') && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    (lines, highlighted)
}

pub(crate) fn render(path: &str, source: &str) -> Option<String> {
    if source.len() > 256 * 1024 {
        return None;
    }
    let lang = language(path)?;
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    if lang == Lang::Html {
        highlight_html(source, &mut out);
        return Some(out);
    }
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut line_start = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            out.push('\n');
            index += 1;
            line_start = true;
            continue;
        }
        if byte.is_ascii_whitespace() {
            out.push(byte as char);
            index += 1;
            continue;
        }
        if let Some(end) = line_comment(lang, source, index, line_start) {
            span(&mut out, "tok-comment", &source[index..end]);
            index = end;
            line_start = false;
            continue;
        }
        if let Some((end, _)) = block_comment(lang, source, index) {
            span(&mut out, "tok-comment", &source[index..end]);
            index = end;
            line_start = false;
            continue;
        }
        if is_quote(lang, byte) {
            let (end, content) = string_end(lang, source, index);
            let class = if is_key_line(lang, content, &source[end..]) { "tok-key" } else { "tok-string" };
            span(&mut out, class, &source[index..end]);
            index = end;
            line_start = false;
            continue;
        }
        if lang == Lang::Shell && byte == b'$' {
            let mut end = index + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > index + 1 {
                span(&mut out, "tok-type", &source[index..end]);
                index = end;
                line_start = false;
                continue;
            }
        }
        if byte.is_ascii_digit() {
            let end = number_end(bytes, index);
            span(&mut out, "tok-number", &source[index..end]);
            index = end;
            line_start = false;
            continue;
        }
        if is_ident_start(byte) {
            let end = ident_end(source, index);
            let word = &source[index..end];
            let next = next_nonspace(bytes, end);
            let class = if keyword(lang, word) {
                Some("tok-keyword")
            } else if is_type(word) {
                Some("tok-type")
            } else if next == Some(b'(') {
                Some("tok-function")
            } else {
                None
            };
            if let Some(class) = class {
                span(&mut out, class, word);
            } else {
                escape_into(&mut out, word);
            }
            index = end;
            line_start = false;
            continue;
        }
        if byte == b'<' {
            escape_into(&mut out, "<");
            index += 1;
        } else if byte == b'&' {
            escape_into(&mut out, "&");
            index += 1;
        } else {
            let ch = source[index..].chars().next().unwrap_or('\u{fffd}');
            out.push(ch);
            index += ch.len_utf8();
        }
        line_start = false;
    }
    Some(out)
}

fn language(path: &str) -> Option<Lang> {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".dockerfile") {
        return Some(Lang::Dockerfile);
    }
    if name == "makefile" || name == "gnumakefile" {
        return Some(Lang::Makefile);
    }
    let extension = name.rsplit_once('.')?.1;
    Some(match extension {
        "rs" => Lang::Rust,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => Lang::JavaScript,
        "py" | "pyi" | "pyw" => Lang::Python,
        "go" => Lang::Go,
        "java" | "kt" | "kts" => Lang::Java,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" => Lang::C,
        "css" | "scss" | "sass" | "less" => Lang::Css,
        "html" | "htm" | "xhtml" | "xml" | "svg" | "vue" => Lang::Html,
        "sh" | "bash" | "zsh" | "fish" | "ksh" => Lang::Shell,
        "sql" => Lang::Sql,
        "yaml" | "yml" => Lang::Yaml,
        "toml" => Lang::Toml,
        "json" | "jsonc" => Lang::Json,
        _ => return None,
    })
}

fn line_comment(lang: Lang, source: &str, index: usize, line_start: bool) -> Option<usize> {
    let bytes = source.as_bytes();
    let rest = &bytes[index..];
    let starts = |prefix: &[u8]| rest.starts_with(prefix);
    let line_end = source[index..].find('\n').map_or(source.len(), |offset| index + offset);
    match lang {
        Lang::Rust | Lang::JavaScript | Lang::Go | Lang::Java | Lang::C => {
            starts(b"//").then_some(line_end)
        }
        Lang::Css => starts(b"//").then_some(line_end),
        Lang::Dockerfile => starts(b"#").then_some(line_end),
        Lang::Python | Lang::Yaml | Lang::Toml | Lang::Makefile | Lang::Shell => {
            (starts(b"#") && (line_start || index == 0 || bytes[index - 1].is_ascii_whitespace())).then_some(line_end)
        }
        Lang::Sql => starts(b"--").then_some(line_end),
        Lang::Json => starts(b"//").then_some(line_end),
        Lang::Html => None,
    }
}

fn block_comment(lang: Lang, source: &str, index: usize) -> Option<(usize, usize)> {
    if !matches!(lang, Lang::Rust | Lang::JavaScript | Lang::Go | Lang::Java | Lang::C | Lang::Css) {
        return None;
    }
    let rest = &source[index..];
    if !rest.starts_with("/*") {
        return None;
    }
    let end = rest.find("*/").map_or(source.len(), |offset| index + offset + 2);
    Some((end, end))
}

fn is_quote(lang: Lang, byte: u8) -> bool {
    match lang {
        Lang::Python => matches!(byte, b'\'' | b'"'),
        Lang::Shell => matches!(byte, b'\'' | b'"'),
        Lang::Sql => matches!(byte, b'\''),
        Lang::Yaml | Lang::Toml => matches!(byte, b'\'' | b'"'),
        Lang::JavaScript => matches!(byte, b'\'' | b'"' | b'`'),
        _ => matches!(byte, b'\'' | b'"'),
    }
}

fn string_end(lang: Lang, source: &str, start: usize) -> (usize, &str) {
    let bytes = source.as_bytes();
    let quote = bytes[start];
    if lang == Lang::Python && start + 2 < bytes.len() && bytes[start + 1] == quote && bytes[start + 2] == quote {
        let triple = [quote, quote, quote];
        let mut index = start + 3;
        while index + 2 < bytes.len() {
            if bytes[index..].starts_with(&triple) {
                return (index + 3, &source[start + 3..index]);
            }
            index += if bytes[index] == b'\\' { 2 } else { 1 };
        }
        return (bytes.len(), &source[start + 3..]);
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return (index + 1, &source[start + 1..index]),
            b'\n' if quote != b'`' => return (index, &source[start + 1..index]),
            _ => index += 1,
        }
    }
    (bytes.len(), &source[start + 1..])
}

fn is_key_line(lang: Lang, content: &str, rest: &str) -> bool {
    matches!(lang, Lang::Json | Lang::Yaml | Lang::Toml)
        && content.len() <= 200
        && rest.trim_start().starts_with(':')
}

fn number_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    let mut exponent = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_') || (exponent && matches!(byte, b'+' | b'-')) {
            exponent = matches!(byte, b'e' | b'E');
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn ident_end(source: &str, start: usize) -> usize {
    let mut index = start;
    while index < source.len() {
        let ch = source[index..].chars().next().unwrap_or('\u{fffd}');
        if ch.is_alphanumeric() || matches!(ch, '_' | '$') || !ch.is_ascii() {
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    index
}

fn next_nonspace(bytes: &[u8], mut index: usize) -> Option<u8> {
    while index < bytes.len() {
        if !bytes[index].is_ascii_whitespace() {
            return Some(bytes[index]);
        }
        index += 1;
    }
    None
}

fn is_type(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && word.chars().any(|ch| ch.is_ascii_lowercase())
}

fn keyword(lang: Lang, word: &str) -> bool {
    const RUST: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "yield",
    ];
    const JS: &[&str] = &[
        "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default",
        "delete", "do", "else", "export", "extends", "false", "finally", "for", "from", "function", "get",
        "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "set", "static", "super",
        "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "with", "yield",
    ];
    const PYTHON: &[&str] = &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
        "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None",
        "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while", "with", "yield",
    ];
    const GO: &[&str] = &[
        "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func",
        "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct",
        "switch", "type", "var",
    ];
    const JAVA: &[&str] = &[
        "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue",
        "default", "do", "double", "else", "enum", "extends", "final", "finally", "float", "for", "goto", "if",
        "implements", "import", "instanceof", "int", "interface", "long", "native", "new", "null", "package",
        "private", "protected", "public", "return", "short", "static", "strictfp", "super", "switch",
        "synchronized", "this", "throw", "throws", "transient", "try", "void", "volatile", "while",
    ];
    const C: &[&str] = &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern",
        "float", "for", "goto", "if", "inline", "int", "long", "register", "restrict", "return", "short", "signed",
        "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile", "while",
        "class", "namespace", "new", "delete", "public", "private", "protected", "template", "using", "virtual",
    ];
    const CSS: &[&str] = &[
        "important", "media", "supports", "keyframes", "import", "font-face", "inherit", "initial", "unset",
        "auto", "none", "flex", "grid", "block", "inline", "absolute", "relative", "fixed", "sticky",
    ];
    const SHELL: &[&str] = &[
        "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "function",
        "in", "select", "time", "coproc", "local", "export", "readonly", "return", "exit", "break", "continue",
    ];
    const SQL: &[&str] = &[
        "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table",
        "index", "view", "drop", "alter", "join", "left", "right", "inner", "outer", "on", "group", "by",
        "order", "having", "limit", "offset", "as", "and", "or", "not", "null", "is", "in", "exists", "case",
        "when", "then", "else", "end", "union", "all", "distinct", "primary", "key", "foreign", "references",
        "default", "unique", "check", "begin", "commit", "rollback", "with",
    ];
    const YAML: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "~"];
    const TOML: &[&str] = &["true", "false"];
    const JSON: &[&str] = &["true", "false", "null"];
    const DOCKER: &[&str] = &[
        "FROM", "RUN", "CMD", "LABEL", "EXPOSE", "ENV", "ADD", "COPY", "ENTRYPOINT", "VOLUME", "USER",
        "WORKDIR", "ARG", "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL", "AS",
    ];
    match lang {
        Lang::Rust => RUST.contains(&word),
        Lang::JavaScript => JS.contains(&word),
        Lang::Python => PYTHON.contains(&word),
        Lang::Go => GO.contains(&word),
        Lang::Java => JAVA.contains(&word),
        Lang::C => C.contains(&word),
        Lang::Css => CSS.contains(&word.to_ascii_lowercase().as_str()),
        Lang::Shell => SHELL.contains(&word),
        Lang::Sql => SQL.contains(&word.to_ascii_lowercase().as_str()),
        Lang::Yaml => YAML.contains(&word),
        Lang::Toml => TOML.contains(&word),
        Lang::Json => JSON.contains(&word),
        Lang::Dockerfile => DOCKER.contains(&word.to_ascii_uppercase().as_str()),
        Lang::Makefile => false,
        Lang::Html => false,
    }
}

fn highlight_html(source: &str, out: &mut String) {
    let mut index = 0;
    while index < source.len() {
        if source[index..].starts_with("<!--") {
            let end = source[index + 4..].find("-->").map_or(source.len(), |offset| index + 4 + offset + 3);
            span(out, "tok-comment", &source[index..end]);
            index = end;
            continue;
        }
        if source[index..].starts_with("<!") {
            let end = source[index..].find('>').map_or(source.len(), |offset| index + offset + 1);
            span(out, "tok-meta", &source[index..end]);
            index = end;
            continue;
        }
        if source.as_bytes()[index] == b'<' && source[index..].chars().nth(1).is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '/') {
            index = html_tag(source, index, out);
            continue;
        }
        let ch = source[index..].chars().next().unwrap_or('\u{fffd}');
        if matches!(ch, '&' | '<' | '>') {
            escape_into(out, ch.encode_utf8(&mut [0; 4]));
        } else {
            out.push(ch);
        }
        index += ch.len_utf8();
    }
}

fn html_tag(source: &str, start: usize, out: &mut String) -> usize {
    let bytes = source.as_bytes();
    out.push_str("&lt;");
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'/') {
        out.push('/');
        index += 1;
    }
    let name_start = index;
    while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'-' | b':' | b'_')) {
        index += 1;
    }
    span(out, "tok-tag", &source[name_start..index]);
    while index < bytes.len() {
        match bytes[index] {
            b'>' => {
                out.push('&');
                out.push_str("gt;");
                return index + 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'>') => {
                out.push_str("/&gt;");
                return index + 2;
            }
            b'=' => {
                out.push('=');
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    out.push(bytes[index] as char);
                    index += 1;
                }
                if index < bytes.len() && matches!(bytes[index], b'"' | b'\'') {
                    let quote = bytes[index];
                    let end = source[index + 1..].find(quote as char).map_or(bytes.len(), |offset| index + 1 + offset + 1);
                    span(out, "tok-string", &source[index..end]);
                    index = end;
                }
                continue;
            }
            b'\'' | b'"' => {
                let quote = bytes[index];
                let end = source[index + 1..].find(quote as char).map_or(bytes.len(), |offset| index + 1 + offset + 1);
                span(out, "tok-string", &source[index..end]);
                index = end;
            }
            byte if byte.is_ascii_whitespace() => {
                out.push(byte as char);
                index += 1;
            }
            byte if byte.is_ascii_alphabetic() || matches!(byte, b'-' | b':' | b'_') => {
                let attr_start = index;
                while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'-' | b':' | b'_' | b'.')) {
                    index += 1;
                }
                span(out, "tok-attr", &source[attr_start..index]);
            }
            _ => {
                let ch = source[index..].chars().next().unwrap_or('\u{fffd}');
                let mut buffer = [0; 4];
                escape_into(out, ch.encode_utf8(&mut buffer));
                index += ch.len_utf8();
            }
        }
    }
    index
}

fn span(out: &mut String, class: &str, text: &str) {
    for (index, segment) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if segment.is_empty() {
            continue;
        }
        out.push_str("<span class=\"");
        out.push_str(class);
        out.push_str("\">");
        escape_into(out, segment);
        out.push_str("</span>");
    }
}

fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}