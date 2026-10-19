//! The reader overlay: a Moodle page or a Teams channel transcript
//! rendered to plain text (SPEC §4.3).

use anyhow::{Context, Result, bail};
use study_graph::GraphClient;
use study_graph::api::Message;
use study_moodle::MoodleClient;

#[derive(Debug, Clone, PartialEq)]
pub struct Reader {
    pub title: String,
    pub lines: Vec<String>,
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
/// is a `— author · time —` header, its body, then its replies indented,
/// and a blank line. Messages without text (member joined, …) are skipped.
pub fn transcript(messages: &[Message], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for message in messages.iter().rev() {
        let body = body_lines(&message.body_html, width);
        if body.is_empty() {
            continue;
        }
        lines.push(format!("— {} —", author_time(message)));
        lines.extend(body);
        for reply in &message.replies {
            let body = body_lines(&reply.body_html, width.saturating_sub(4));
            if body.is_empty() {
                continue;
            }
            lines.push(format!("    ↳ {}", author_time(reply)));
            lines.extend(body.into_iter().map(|line| format!("    {line}")));
        }
        lines.push(String::new());
    }
    lines
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
fn body_lines(html: &str, width: usize) -> Vec<String> {
    let mut lines = html_to_lines(html, width);
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Plain-text lines of `html`, wrapped to `width`; unrenderable HTML
/// becomes a one-line note rather than an error.
pub fn html_to_lines(html: &str, width: usize) -> Vec<String> {
    let text = match html2text::from_read(html.as_bytes(), width.max(10)) {
        Ok(text) => text,
        Err(err) => format!("(cannot render page: {err})"),
    };
    text.lines().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_lists_and_links() {
        let html = "<h1>Welcome</h1><ul><li>one</li><li>two</li></ul>\
                    <p>See <a href=\"https://example.org\">the site</a>.</p>";
        let lines = html_to_lines(html, 60);
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|line| line.contains("Welcome")));
        assert!(lines.iter().any(|line| line.contains("one")));
        assert!(lines.iter().any(|line| line.contains("the site")));
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
            lines,
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
            lines: (0..10).map(|i| i.to_string()).collect(),
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
