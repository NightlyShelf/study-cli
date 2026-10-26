//! The reader overlay: a Moodle page or a Teams channel transcript
//! rendered to styled text (SPEC §4.3).

use anyhow::{Context, Result, bail};
use html2text::render::{RichAnnotation, TaggedLine};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use study_graph::GraphClient;
use study_graph::api::Message;
use study_moodle::MoodleClient;

#[derive(Debug, Clone, PartialEq)]
pub struct Reader {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
    pub web_url: Option<String>,
}

impl Reader {
    /// Scrolls by `delta` lines, never past the last page of `height` lines.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max = self.lines.len().saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    pub fn scroll_to_end(&mut self, height: usize) {
        self.scroll = self.lines.len().saturating_sub(height);
    }
}

/// Columns of text in the reader box: at most 80 so lines stay readable
/// on a wide terminal, and on a narrow one whatever is left after the
/// border and padding.
pub fn text_width(terminal_width: u16) -> usize {
    usize::from(terminal_width).saturating_sub(8).clamp(20, 80)
}

/// Lines of text in the reader box: a box 90 % of the terminal high, minus
/// its border and the padding line above the text. Both the drawing and the
/// paging keys use this, so they agree on what one page is.
pub fn reader_height(terminal_height: u16) -> usize {
    (usize::from(terminal_height) * 9 / 10)
        .saturating_sub(3)
        .max(1)
}

/// Fetches the page of module `cmid` in course `course_remote_id` and
/// renders it wrapped to `width` columns. The api returns every page of
/// the course at once; that is still one cheap call.
pub async fn load_page(
    client: &MoodleClient,
    course_remote_id: u64,
    cmid: u64,
    width: usize,
) -> Result<Reader> {
    let pages = client
        .pages(&[course_remote_id])
        .await
        .context("fetching pages")?;
    let Some(page) = pages.into_iter().find(|page| page.coursemodule == cmid) else {
        bail!("page {cmid} not found in course {course_remote_id}");
    };
    let html = if page.content.is_empty() {
        page.intro
    } else {
        page.content
    };
    Ok(Reader {
        title: page.name,
        lines: html_to_lines(&html, width),
        scroll: 0,
        web_url: None,
    })
}

/// Fetches the newest 50 messages of a channel as a transcript wrapped
/// to `width` columns. The caller names the reader: the ids say nothing.
pub async fn load_messages(
    graph: &GraphClient,
    team_id: &str,
    channel_id: &str,
    width: usize,
) -> Result<Reader> {
    let messages = graph
        .messages(team_id, channel_id, 50)
        .await
        .context("fetching messages")?;
    Ok(Reader {
        title: String::new(),
        lines: transcript(&messages, width),
        scroll: 0,
        web_url: None,
    })
}

/// Graph sends newest first; a transcript reads oldest first. Each message
/// is a bold `— author · time —` header, its body, then its replies indented
/// under a dim header, and a blank line. Messages without text (member
/// joined, …) are skipped.
pub fn transcript(messages: &[Message], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in messages.iter().rev() {
        let body = body_lines(&message.body_html, width);
        if body.is_empty() {
            continue;
        }
        lines.push(Line::from(format!("— {} —", author_time(message))).bold());
        lines.extend(body);
        for reply in &message.replies {
            let body = body_lines(&reply.body_html, width.saturating_sub(4));
            if body.is_empty() {
                continue;
            }
            lines.push(Line::from(format!("    ↳ {}", author_time(reply))).dim());
            lines.extend(body.into_iter().map(indent));
        }
        lines.push(Line::default());
    }
    lines
}

fn indent(mut line: Line<'static>) -> Line<'static> {
    line.spans.insert(0, Span::raw("    "));
    line
}

fn author_time(message: &Message) -> String {
    let author = if message.author.is_empty() {
        "unknown"
    } else {
        &message.author
    };
    format!("{author} · {}", short_time(&message.created))
}

/// `2026-10-19T12:34:56.789Z` → `2026-10-19 12:34`; anything else as is.
fn short_time(rfc3339: &str) -> String {
    match (rfc3339.get(..10), rfc3339.get(10..11), rfc3339.get(11..16)) {
        (Some(date), Some("T"), Some(time)) => format!("{date} {time}"),
        _ => rfc3339.to_string(),
    }
}

/// `html_to_lines` without the trailing blank lines html2text leaves, so an
/// empty body renders to no lines at all.
fn body_lines(html: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = html_to_lines(html, width);
    while lines.last().is_some_and(is_blank) {
        lines.pop();
    }
    lines
}

/// Styled lines of `html`, wrapped to `width`: bold, italic, links and code
/// become terminal styles rather than markdown-like markup. Unrenderable
/// HTML becomes a one-line note rather than an error.
pub fn html_to_lines(html: &str, width: usize) -> Vec<Line<'static>> {
    let rendered = html2text::config::rich()
        .link_footnotes(false)
        .lines_from_read(html.as_bytes(), width.max(10));
    let tagged = match rendered {
        Ok(tagged) => tagged,
        Err(err) => return vec![Line::from(format!("(cannot render page: {err})"))],
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut after_heading = false;
    for tagged in &tagged {
        let text: String = tagged
            .tagged_strings()
            .map(|piece| piece.s.as_str())
            .collect();
        let heading = heading_level(&text);
        let blank = text.trim().is_empty();
        // Runs of blank lines are noise; one blank line under a heading is air.
        if blank && lines.last().is_some_and(is_blank) {
            continue;
        }
        if after_heading && !blank && heading.is_none() {
            lines.push(Line::default());
        }
        lines.push(match heading {
            Some(level) => heading_line(&text, level),
            None => styled_line(tagged),
        });
        after_heading = heading.is_some();
    }
    lines
}

fn is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// The rich decorator still prefixes headings with `# `…`###### `; the
/// level, or None when the line is not a heading.
fn heading_level(text: &str) -> Option<usize> {
    let level = text.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&level) && text.as_bytes().get(level) == Some(&b' ') {
        Some(level)
    } else {
        None
    }
}

/// The heading without its `#` prefix, bold, and yellow for the top levels.
fn heading_line(text: &str, level: usize) -> Line<'static> {
    let style = if level <= 2 {
        Style::new().bold().fg(Color::Yellow)
    } else {
        Style::new().bold()
    };
    Line::styled(text[level + 1..].to_string(), style)
}

fn styled_line(tagged: &TaggedLine<Vec<RichAnnotation>>) -> Line<'static> {
    let spans: Vec<Span<'static>> = tagged
        .tagged_strings()
        .map(|piece| Span::styled(piece.s.clone(), style(&piece.tag)))
        .collect();
    Line::from(spans)
}

fn style(tags: &[RichAnnotation]) -> Style {
    let mut style = Style::new();
    for tag in tags {
        style = match tag {
            RichAnnotation::Strong => style.bold(),
            RichAnnotation::Emphasis => style.italic(),
            RichAnnotation::Link(_) => style.underlined().fg(Color::Blue),
            RichAnnotation::Code | RichAnnotation::Preformat(_) => style.fg(Color::Cyan),
            RichAnnotation::Strikeout => style.crossed_out(),
            RichAnnotation::Image(_) => style.dim(),
            // Page authors' colours rarely suit a terminal theme.
            _ => style,
        };
    }
    style
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    fn span<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content == text)
            .unwrap_or_else(|| panic!("no span {text:?} in {:?}", texts(lines)))
    }

    #[test]
    fn headings_links_and_bold_become_styles_not_markup() {
        let html = "<h1>Welcome</h1><p>See <a href=\"https://x\">the site</a> \
                    and <strong>bold</strong></p>";
        let lines = html_to_lines(html, 60);
        let text = texts(&lines);
        assert_eq!(text[0], "Welcome");
        assert!(
            text.iter()
                .all(|line| !line.contains('#') && !line.contains("[1]"))
        );
        assert!(text.iter().all(|line| !line.contains("https://x")));
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[0].style.fg, Some(Color::Yellow));
        assert!(is_blank(&lines[1]));
        let link = span(&lines, "the site");
        assert!(link.style.add_modifier.contains(Modifier::UNDERLINED));
        let bold = span(&lines, "bold");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn lists_keep_their_bullets() {
        let lines = html_to_lines("<ul><li>one</li><li>two</li></ul>", 60);
        assert_eq!(texts(&lines)[..2], ["* one", "* two"]);
    }

    #[test]
    fn blank_lines_collapse_to_one() {
        let html = "<p>a</p><br><br><br><p>b</p><h3>c</h3><p>d</p>";
        let lines = html_to_lines(html, 60);
        let text = texts(&lines);
        let blanks = text
            .windows(2)
            .filter(|pair| pair[0].trim().is_empty() && pair[1].trim().is_empty());
        assert_eq!(blanks.count(), 0, "{text:?}");
        assert!(text.contains(&"a".to_string()));
        assert!(text.contains(&"c".to_string()));
        assert!(!text.iter().any(|line| line.starts_with('#')));
    }

    #[test]
    fn text_width_caps_at_eighty_and_leaves_room_for_the_box() {
        assert_eq!(text_width(200), 80);
        assert_eq!(text_width(60), 52);
        assert_eq!(text_width(10), 20);
    }

    #[test]
    fn reader_height_leaves_room_for_border_and_padding() {
        assert_eq!(reader_height(30), 24);
        assert_eq!(reader_height(3), 1);
    }

    fn message(id: &str, author: &str, created: &str, body_html: &str) -> Message {
        Message {
            id: id.to_string(),
            author: author.to_string(),
            created: created.to_string(),
            body_html: body_html.to_string(),
            replies: Vec::new(),
        }
    }

    #[test]
    fn transcript_reads_oldest_first_with_indented_replies() {
        let mut exam = message(
            "2",
            "Ada Lovelace",
            "2026-10-18T10:00:00.123Z",
            "<p>Exam on Friday</p>",
        );
        exam.replies
            .push(message("3", "Bob", "2026-10-18T10:05:00Z", "<p>Thanks</p>"));
        let joined = message("4", "", "2026-10-18T11:00:00Z", "<systemEventMessage/>");
        let hello = message("1", "Bob", "2026-10-17T09:30:00Z", "hello all");
        // Newest first, as Graph sends them; the empty one must vanish.
        let lines = transcript(&[joined, exam, hello], 60);
        assert_eq!(
            texts(&lines),
            [
                "— Bob · 2026-10-17 09:30 —",
                "hello all",
                "",
                "— Ada Lovelace · 2026-10-18 10:00 —",
                "Exam on Friday",
                "    ↳ Bob · 2026-10-18 10:05",
                "    Thanks",
                "",
            ]
        );
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
        assert!(lines[5].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn short_time_keeps_odd_values() {
        assert_eq!(short_time("2026-10-19T12:34:56.789Z"), "2026-10-19 12:34");
        assert_eq!(short_time(""), "");
        assert_eq!(short_time("yesterday"), "yesterday");
    }

    #[test]
    fn scrolling_stays_inside_the_text() {
        let mut reader = Reader {
            title: "t".to_string(),
            lines: (0..10).map(|i| Line::from(i.to_string())).collect(),
            scroll: 0,
            web_url: None,
        };
        reader.scroll_by(100, 4);
        assert_eq!(reader.scroll, 6);
        reader.scroll_by(-100, 4);
        assert_eq!(reader.scroll, 0);
        reader.scroll_to_end(4);
        assert_eq!(reader.scroll, 6);
        reader.scroll_to_end(50);
        assert_eq!(reader.scroll, 0);
    }
}
