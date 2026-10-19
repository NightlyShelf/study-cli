//! The reader overlay: a Moodle page rendered to plain text (SPEC §4.3).

use anyhow::{Context, Result, bail};
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
