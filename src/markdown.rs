use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

/// Markdown for issue bodies and READMEs. Raw HTML is escaped and script-capable
/// link/image schemes are dropped: this content shares the forge origin.
pub fn render(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(text, options).map(|event| match event {
        Event::Html(raw) => Event::Text(raw),
        Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe_destination(&dest_url),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: safe_destination(&dest_url),
            title,
            id,
        }),
        other => other,
    });

    let mut output = String::with_capacity(text.len());
    html::push_html(&mut output, parser);
    output
}

fn safe_destination(destination: &str) -> CowStr<'static> {
    let normalized: String = destination
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
        .flat_map(char::to_lowercase)
        .collect();
    let blocked = ["javascript:", "vbscript:", "data:", "file:"]
        .iter()
        .any(|scheme| normalized.starts_with(scheme));
    if blocked {
        CowStr::Borrowed("#")
    } else {
        CowStr::from(destination.to_string())
    }
}
