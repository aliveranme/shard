use crate::{models::Repo, util};
use html5gum::{Token, Tokenizer};
use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use std::collections::{hash_map::Entry, HashMap, VecDeque};
use std::fmt::Write;
use std::iter;
use worker::Url;

pub(crate) struct Context<'a> {
    repo: &'a Repo,
    reference: &'a str,
    directory: &'a str,
}

impl<'a> Context<'a> {
    pub(crate) fn new(repo: &'a Repo, reference: &'a str, path: &'a str) -> Self {
        Self {
            repo,
            reference,
            directory: path.rsplit_once('/').map_or("", |(dir, _)| dir),
        }
    }

    pub(crate) fn image_url(&self, path: &str) -> String {
        self.resource_url("media", path)
    }

    fn resource_url(&self, surface: &str, path: &str) -> String {
        let owner = &self.repo.owner_name;
        let repo = &self.repo.name;
        let branch = urlencoding::encode(self.reference);
        if path.is_empty() {
            format!("/{owner}/{repo}/{surface}?branch={branch}")
        } else {
            let path = urlencoding::encode(path);
            format!("/{owner}/{repo}/{surface}/{path}?branch={branch}")
        }
    }

    fn destination<'m>(&self, destination: CowStr<'m>, image: bool) -> CowStr<'m> {
        let blocked = CowStr::Borrowed("#");
        if destination.contains('\\') || destination.chars().any(char::is_control) {
            return blocked;
        }
        if destination.starts_with("//") {
            return blocked;
        }
        let (target, fragment) = destination.split_once('#').unwrap_or((&destination, ""));
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path
            .split('/')
            .next()
            .is_some_and(|first| first.contains(':'))
        {
            return if absolute_url(&destination, image) {
                destination
            } else {
                blocked
            };
        }
        if path.is_empty() {
            return if image { blocked } else { destination };
        }
        let Some(path) = repo_path(self.directory, path) else {
            return blocked;
        };
        let mut url = if image {
            if path.is_empty() {
                return blocked;
            }
            self.image_url(&path)
        } else {
            self.resource_url("src", &path)
        };
        if !query.is_empty() {
            url.push('&');
            url.push_str(query);
        }
        if !fragment.is_empty() {
            url.push('#');
            url.push_str(fragment);
        }
        CowStr::from(url)
    }
}

pub(crate) fn render_document(text: &str, context: &Context<'_>) -> String {
    render_markdown(text, context, true)
}

/// Render CommonMark with filtered HTML images and escape other raw markup.
pub(crate) fn render(text: &str, context: &Context<'_>) -> String {
    render_markdown(text, context, false)
}

fn render_markdown(text: &str, context: &Context<'_>, document: bool) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut source = Parser::new_ext(text, options);
    let mut heading = VecDeque::new();
    let mut ids = HashMap::new();
    let mut input = iter::from_fn(|| {
        if let Some(event) = heading.pop_front() {
            return Some(event);
        }
        Some(match source.next()? {
            Event::Start(Tag::Heading {
                level,
                classes,
                attrs,
                ..
            }) if document => {
                let id = heading_id(&mut source, &mut heading, &mut ids);
                Event::Start(Tag::Heading {
                    level,
                    id,
                    classes,
                    attrs,
                })
            }
            event => event,
        })
    })
    .peekable();
    let mut pending = VecDeque::new();
    let mut skip_autolinks = false;
    let events = iter::from_fn(|| {
        if let Some(event) = pending.pop_front() {
            return Some(event);
        }
        let event = match input.next()? {
            Event::Html(raw) if starts_with_image(&raw) => {
                // CommonMark can emit an HTML block one line at a time.
                let raw = if matches!(input.peek(), Some(Event::Html(_))) {
                    let mut joined = String::from(raw.as_ref());
                    while let Some(Event::Html(next)) =
                        input.next_if(|event| matches!(event, Event::Html(_)))
                    {
                        joined.push_str(&next);
                    }
                    CowStr::from(joined)
                } else {
                    raw
                };
                html_image_event(raw, context)
            }
            Event::InlineHtml(raw) if starts_with_image(&raw) => html_image_event(raw, context),
            Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                skip_autolinks = true;
                let dest_url = if link_type == LinkType::Email {
                    dest_url
                } else {
                    context.destination(dest_url, false)
                };
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                })
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let start = Event::Start(Tag::Image {
                    link_type,
                    dest_url: context.destination(dest_url, true),
                    title,
                    id,
                });
                image_event(start, &mut input)
            }
            event @ Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                skip_autolinks = true;
                event
            }
            event @ Event::End(TagEnd::Link | TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                skip_autolinks = false;
                event
            }
            Event::Text(text) if !skip_autolinks => {
                // Entity references and punctuation can split plain text into adjacent events.
                let text = if matches!(input.peek(), Some(Event::Text(_))) {
                    let mut joined = String::from(text.as_ref());
                    while let Some(Event::Text(next)) =
                        input.next_if(|event| matches!(event, Event::Text(_)))
                    {
                        joined.push_str(&next);
                    }
                    CowStr::from(joined)
                } else {
                    text
                };
                return Some(autolinks(text, context, &mut pending));
            }
            event => event,
        };
        Some(event)
    });
    let mut output = String::with_capacity(text.len());
    html::push_html(&mut output, events);
    output
}

fn heading_id<'a>(
    source: &mut impl Iterator<Item = Event<'a>>,
    heading: &mut VecDeque<Event<'a>>,
    ids: &mut HashMap<String, usize>,
) -> Option<CowStr<'a>> {
    let mut slug = String::new();
    for event in source {
        match &event {
            Event::Text(text) | Event::Code(text) => {
                for ch in text.chars() {
                    if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                        slug.extend(ch.to_lowercase());
                    } else if ch.is_whitespace() {
                        slug.push('-');
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => slug.push('-'),
            _ => {}
        }
        let end = matches!(event, Event::End(TagEnd::Heading(_)));
        heading.push_back(event);
        if end {
            break;
        }
    }
    if slug.is_empty() {
        return None;
    }
    if let Some(mut suffix) = ids.get(&slug).copied() {
        loop {
            suffix += 1;
            let candidate = format!("{slug}-{suffix}");
            if let Entry::Vacant(entry) = ids.entry(candidate.clone()) {
                entry.insert(0);
                ids.insert(slug, suffix);
                return Some(CowStr::from(candidate));
            }
        }
    }
    ids.insert(slug.clone(), 0);
    Some(CowStr::from(slug))
}

fn image_event<'a>(start: Event<'a>, input: &mut impl Iterator<Item = Event<'a>>) -> Event<'a> {
    let mut depth = 1;
    let rest = iter::from_fn(|| {
        if depth == 0 {
            return None;
        }
        Some(match input.next()? {
            event @ Event::Start(Tag::Image { .. }) => {
                depth += 1;
                event
            }
            event @ Event::End(TagEnd::Image) => {
                depth -= 1;
                event
            }
            Event::Html(text) | Event::InlineHtml(text) => Event::Text(text),
            event => event,
        })
    });
    let mut output = String::new();
    append_image(&mut output, iter::once(start).chain(rest), None, None);
    Event::InlineHtml(CowStr::from(output))
}

fn starts_with_image(raw: &str) -> bool {
    let raw = raw.trim_start();
    raw.get(..4)
        .is_some_and(|name| name.eq_ignore_ascii_case("<img"))
        && raw
            .as_bytes()
            .get(4)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>' || *byte == b'/')
}

fn html_image_event<'a>(raw: CowStr<'a>, context: &Context<'_>) -> Event<'a> {
    let mut output = String::new();
    for token in Tokenizer::new(raw.as_ref()) {
        let tag = match token {
            Ok(Token::StartTag(tag)) if tag.name == b"img" => tag,
            Ok(Token::String(text)) if text.iter().all(u8::is_ascii_whitespace) => {
                output.push_str(&String::from_utf8_lossy(&text));
                continue;
            }
            _ => return Event::Text(raw),
        };
        let attribute = |name: &[u8]| {
            tag.attributes
                .get(name)
                .and_then(|value| std::str::from_utf8(&value.value).ok())
        };
        let Some(src) = attribute(b"src")
            .map(str::trim)
            .filter(|src| !src.is_empty())
        else {
            return Event::Text(raw);
        };
        let dest_url = context.destination(CowStr::Borrowed(src), true);
        if dest_url.as_ref() == "#" {
            return Event::Text(raw);
        }
        let dimension = |name| attribute(name).and_then(|value| value.parse::<u16>().ok());
        append_image(
            &mut output,
            [
                Event::Start(Tag::Image {
                    link_type: LinkType::Inline,
                    dest_url,
                    title: CowStr::Borrowed(attribute(b"title").unwrap_or_default()),
                    id: CowStr::Borrowed(""),
                }),
                Event::Text(CowStr::Borrowed(attribute(b"alt").unwrap_or_default())),
                Event::End(TagEnd::Image),
            ],
            dimension(b"width"),
            dimension(b"height"),
        );
    }
    Event::InlineHtml(CowStr::from(output))
}

fn append_image<'a>(
    output: &mut String,
    events: impl IntoIterator<Item = Event<'a>>,
    width: Option<u16>,
    height: Option<u16>,
) {
    // The CommonMark serializer escapes the src, alt and title attributes.
    html::push_html(output, events.into_iter());
    if !output.ends_with(" />") {
        return;
    }
    output.truncate(output.len() - " />".len());
    output.push_str(
        r#" class="repo-image" loading="lazy" decoding="async" referrerpolicy="no-referrer""#,
    );
    if let Some(width) = width {
        let _ = write!(output, " width=\"{width}\"");
    }
    if let Some(height) = height {
        let _ = write!(output, " height=\"{height}\"");
    }
    output.push_str(" />");
}

fn repo_path(directory: &str, path: &str) -> Option<String> {
    let path = urlencoding::decode(path).ok()?;
    if path.contains('\\') || path.chars().any(char::is_control) {
        return None;
    }
    let base = if path.starts_with('/') { "" } else { directory };
    let mut segments = Vec::new();
    for segment in base.split('/').chain(path.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            segment => segments.push(segment),
        }
    }
    Some(segments.join("/"))
}

fn absolute_url(destination: &str, image: bool) -> bool {
    let Some((scheme, target)) = destination.split_once(':') else {
        return false;
    };
    if !image
        && ["mailto", "tel"]
            .iter()
            .any(|candidate| scheme.eq_ignore_ascii_case(candidate))
    {
        return Url::parse(destination).is_ok();
    }
    let allowed = scheme.eq_ignore_ascii_case("https")
        || (!image
            && ["http", "ftp"]
                .iter()
                .any(|candidate| scheme.eq_ignore_ascii_case(candidate)));
    let has_authority = target
        .strip_prefix("//")
        .is_some_and(|authority| !authority.starts_with('/'));
    allowed && has_authority && Url::parse(destination).is_ok_and(|url| url.has_host())
}

fn autolinks<'a>(
    text: CowStr<'a>,
    context: &Context<'_>,
    pending: &mut VecDeque<Event<'a>>,
) -> Event<'a> {
    let mut tail = text.as_ref();
    let mut plain_start = 0;
    let mut previous = None;
    while let Some(first) = tail.chars().next() {
        if previous.is_none_or(|c| !name_char(c) && !matches!(c, '/' | '#' | '@')) {
            if let Some((label, destination)) = autolink_at(tail, context) {
                let offset = text.len() - tail.len();
                if plain_start < offset {
                    pending.push_back(Event::Text(text_slice(&text, plain_start, offset)));
                }
                pending.push_back(Event::Start(Tag::Link {
                    link_type: LinkType::Autolink,
                    dest_url: CowStr::from(destination),
                    title: CowStr::Borrowed(""),
                    id: CowStr::Borrowed(""),
                }));
                plain_start = offset + label.len();
                pending.push_back(Event::Text(text_slice(&text, offset, plain_start)));
                pending.push_back(Event::End(TagEnd::Link));
                previous = label.chars().next_back();
                tail = &tail[label.len()..];
                continue;
            }
        }
        previous = Some(first);
        tail = &tail[first.len_utf8()..];
    }
    if pending.is_empty() {
        return Event::Text(text);
    }
    if plain_start < text.len() {
        pending.push_back(Event::Text(text_slice(&text, plain_start, text.len())));
    }
    pending
        .pop_front()
        .expect("at least one autolink was queued")
}

fn text_slice<'a>(text: &CowStr<'a>, start: usize, end: usize) -> CowStr<'a> {
    match text {
        CowStr::Borrowed(text) => CowStr::Borrowed(&(*text)[start..end]),
        _ => CowStr::from(text[start..end].to_owned()),
    }
}

fn autolink_at<'a>(text: &'a str, context: &Context<'_>) -> Option<(&'a str, String)> {
    if ["https://", "http://"].iter().any(|scheme| {
        text.get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    }) {
        let url = plain_url(text);
        return absolute_url(url, false).then(|| (url, url.to_owned()));
    }
    let (owner, repo, digits) = if let Some(reference) = qualified_reference(text) {
        reference
    } else {
        let digits = if let Some(digits) = text.strip_prefix('#') {
            digits
        } else if text
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GH-"))
        {
            &text[3..]
        } else {
            return None;
        };
        (
            context.repo.owner_name.as_str(),
            context.repo.name.as_str(),
            digits,
        )
    };
    let end = digits
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(digits.len());
    let (number, suffix) = digits.split_at(end);
    let invalid_suffix = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '#');
    if suffix.starts_with(invalid_suffix)
        || suffix
            .strip_prefix('.')
            .is_some_and(|rest| rest.starts_with(invalid_suffix))
    {
        return None;
    }
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|number| (1..=9_007_199_254_740_991).contains(number))?;
    let label = &text[..text.len() - suffix.len()];
    Some((label, format!("/{owner}/{repo}/issues/{number}")))
}

fn qualified_reference(text: &str) -> Option<(&str, &str, &str)> {
    let (owner, rest) = text.split_at(text.find(|c| !name_char(c))?);
    let rest = rest.strip_prefix('/')?;
    let (repo, rest) = rest.split_at(rest.find(|c| !name_char(c))?);
    let digits = rest.strip_prefix('#')?;
    (util::valid_slug(owner) && util::valid_slug(repo)).then_some((owner, repo, digits))
}

fn name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_')
}

fn plain_url(text: &str) -> &str {
    let mut parens = 0;
    let mut brackets = 0;
    let mut end = text.len();
    for (index, ch) in text.char_indices() {
        match ch {
            ch if ch.is_whitespace() || ch.is_control() => {
                end = index;
                break;
            }
            '<' | '>' | '"' | '\'' | '\\' | '\x60' => {
                end = index;
                break;
            }
            '，' | '。' | '；' | '！' | '？' | '、' | '）' | '》' | '】' => {
                end = index;
                break;
            }
            '(' => parens += 1,
            '[' => brackets += 1,
            ')' if parens == 0 => {
                end = index;
                break;
            }
            ']' if brackets == 0 => {
                end = index;
                break;
            }
            ')' => parens -= 1,
            ']' => brackets -= 1,
            _ => {}
        }
    }
    text[..end].trim_end_matches(['.', ',', ':', ';', '!', '?'])
}
