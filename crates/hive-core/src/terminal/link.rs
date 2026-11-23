//! Web links under a terminal cell: OSC 8 hyperlinks first, then plain `http(s)://` text.
//!
//! Only web addresses count. Terminal output is untrusted, and handing `file:`, `ssh:` or a
//! custom scheme to the system opener could launch an arbitrary app on a click.

use std::ops::RangeInclusive;
use std::sync::{Mutex, PoisonError};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

use super::selection::grid_point;
use super::{Point, Terminal};

/// Characters a URL cannot contain, after Alacritty's default hint pattern. Lowercase only:
/// `RegexSearch` turns case-insensitive matching off when the pattern has an uppercase letter.
const URL_PATTERN: &str = r#"https?://[^\x00-\x1f\x7f-\x9f<>"\s{}\^⟨⟩`\\]+"#;

/// How far a URL may reach from the hovered cell. The lookup runs on every render under the
/// terminal lock, and one wrapped line (minified output) can fill the whole scrollback:
/// unbounded, that is a regex pass over a million cells per frame.
const URL_ROWS: i32 = 100;

/// A web link covering a run of grid cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLink {
    url: String,
    cells: RangeInclusive<Point>,
}

impl TerminalLink {
    /// The `http` or `https` address to open.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether a grid cell, as `RenderableContent` reports it, belongs to the link.
    pub fn contains(&self, point: Point) -> bool {
        self.cells.contains(&point)
    }
}

/// The URL search automaton, compiled on the first lookup and kept for the terminal's life.
/// Boxed: its four lazy DFAs would otherwise make every `Terminal` value large.
#[derive(Default)]
pub(super) struct UrlSearch(Mutex<Option<Box<RegexSearch>>>);

impl Terminal {
    /// The web link at a visible cell, accounting for scrollback.
    pub fn link_at(&self, point: Point<usize>) -> Option<TerminalLink> {
        let term = self.term.lock();
        let point = grid_point(&term, point);
        hyperlink_at(&term, point).or_else(|| self.url_search.find(&term, point))
    }
}

impl UrlSearch {
    fn find<T>(&self, term: &Term<T>, point: Point) -> Option<TerminalLink> {
        let mut regex = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let regex = match &mut *regex {
            Some(regex) => regex,
            empty => empty.insert(Box::new(
                RegexSearch::new(URL_PATTERN)
                    .inspect_err(|error| tracing::warn!(%error, "terminal URL pattern rejected"))
                    .ok()?,
            )),
        };
        url_at(term, point, regex)
    }
}

/// An OSC 8 hyperlink: the run of cells on this line that carries the same link.
fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<TerminalLink> {
    let hyperlink = term.grid()[point].hyperlink()?;
    if !is_web_url(hyperlink.uri()) {
        return None;
    }
    let same = |cell: Point| term.grid()[cell].hyperlink().as_ref() == Some(&hyperlink);
    let (line_start, line_end) = (term.line_search_left(point), term.line_search_right(point));
    let mut start = point;
    while start > line_start && same(start.sub(term, Boundary::Grid, 1)) {
        start = start.sub(term, Boundary::Grid, 1);
    }
    let mut end = point;
    while end < line_end && same(end.add(term, Boundary::Grid, 1)) {
        end = end.add(term, Boundary::Grid, 1);
    }
    Some(TerminalLink {
        url: hyperlink.uri().to_owned(),
        cells: start..=end,
    })
}

/// A URL in the text of the (soft-wrapped) line under `point`, looked for no further than
/// [`URL_ROWS`] rows either side of it.
fn url_at<T>(term: &Term<T>, point: Point, regex: &mut RegexSearch) -> Option<TerminalLink> {
    let line_start = term
        .line_search_left(point)
        .max(Point::new(point.line - URL_ROWS, Column(0)));
    let line_end = term
        .line_search_right(point)
        .min(Point::new(point.line + URL_ROWS, term.last_column()));
    let found = RegexIter::new(line_start, line_end, Direction::Right, term, regex)
        .take_while(|found| *found.start() <= point)
        .find(|found| found.contains(&point))?;
    let text = term.bounds_to_string(*found.start(), *found.end());
    let url = trim_url(&text);
    if !is_web_url(url) {
        return None;
    }
    // Trimmed characters are ASCII, one cell each.
    let trimmed = text[url.len()..].chars().count();
    let last = found.end().sub(term, Boundary::Grid, trimmed);
    (point <= last).then(|| TerminalLink {
        url: url.to_owned(),
        cells: *found.start()..=last,
    })
}

/// Drops the punctuation that ends a sentence around a URL, and a closing bracket that has no
/// opening one inside it: `(see https://a.b/c).` → `https://a.b/c`, `https://a.b/f(x)` stays.
fn trim_url(url: &str) -> &str {
    let mut url = url;
    while let Some(last) = url.chars().next_back() {
        let unbalanced = |open: char| url.matches(open).count() < url.matches(last).count();
        let trim = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"' | '*' => true,
            ')' => unbalanced('('),
            ']' => unbalanced('['),
            _ => false,
        };
        if !trim {
            break;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
    url
}

/// `http://` or `https://` (any case) followed by at least a host character.
fn is_web_url(url: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
            && url.len() > scheme.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalSize;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::Column;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn term(text: &str, cols: u16, rows: u16) -> Term<VoidListener> {
        let mut term = Term::new(
            Config::default(),
            &TerminalSize::new(cols, rows),
            VoidListener,
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn link(term: &Term<VoidListener>, row: usize, col: usize) -> Option<TerminalLink> {
        let point = grid_point(term, Point::new(row, Column(col)));
        let mut regex = RegexSearch::new(URL_PATTERN).unwrap();
        hyperlink_at(term, point).or_else(|| url_at(term, point, &mut regex))
    }

    fn url(term: &Term<VoidListener>, row: usize, col: usize) -> Option<String> {
        link(term, row, col).map(|link| link.url)
    }

    #[test]
    fn finds_a_url_from_any_of_its_cells_and_nothing_beside_it() {
        let term = term("see https://example.com/a?b=1#c now", 40, 2);
        for col in [4, 12, 30] {
            assert_eq!(
                url(&term, 0, col).as_deref(),
                Some("https://example.com/a?b=1#c")
            );
        }
        assert_eq!(url(&term, 0, 3), None);
        assert_eq!(url(&term, 0, 32), None);
        let found = link(&term, 0, 4).unwrap();
        assert!(found.contains(Point::new(alacritty_terminal::index::Line(0), Column(30))));
        assert!(!found.contains(Point::new(alacritty_terminal::index::Line(0), Column(31))));
    }

    #[test]
    fn picks_the_url_under_the_pointer_among_several() {
        let term = term("http://one.test and HTTPS://two.test", 40, 2);
        assert_eq!(url(&term, 0, 2).as_deref(), Some("http://one.test"));
        assert_eq!(url(&term, 0, 25).as_deref(), Some("HTTPS://two.test"));
        assert_eq!(url(&term, 0, 17), None);
    }

    #[test]
    fn follows_a_url_across_a_soft_wrap() {
        let term = term("go https://example.com/long/path", 16, 3);
        assert_eq!(term.columns(), 16);
        assert_eq!(
            url(&term, 1, 5).as_deref(),
            Some("https://example.com/long/path")
        );
        assert_eq!(
            url(&term, 0, 4).as_deref(),
            Some("https://example.com/long/path")
        );
    }

    #[test]
    fn hard_line_breaks_end_a_url() {
        let term = term("https://a.test/x\r\nmore", 40, 3);
        assert_eq!(url(&term, 0, 0).as_deref(), Some("https://a.test/x"));
        assert_eq!(url(&term, 1, 0), None);
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        let term = term("(see https://a.test/f(x)).", 40, 2);
        let found = link(&term, 0, 6).unwrap();
        assert_eq!(found.url(), "https://a.test/f(x)");
        let line = alacritty_terminal::index::Line(0);
        assert!(found.contains(Point::new(line, Column(23))));
        assert!(!found.contains(Point::new(line, Column(24))));
        assert_eq!(url(&term, 0, 24), None);
    }

    #[test]
    fn ignores_other_schemes_and_bare_schemes() {
        for text in [
            "file:///etc/passwd",
            "ssh://host",
            "https://",
            "x-app://open",
        ] {
            assert_eq!(url(&term(text, 40, 2), 0, 2), None, "{text}");
        }
    }

    #[test]
    fn osc8_hyperlink_opens_its_target_not_its_label() {
        let term = term(
            "a \x1b]8;;https://target.test/x\x1b\\label\x1b]8;;\x1b\\ b",
            40,
            2,
        );
        let found = link(&term, 0, 4).unwrap();
        assert_eq!(found.url(), "https://target.test/x");
        let line = alacritty_terminal::index::Line(0);
        assert!(found.contains(Point::new(line, Column(2))));
        assert!(found.contains(Point::new(line, Column(6))));
        assert!(!found.contains(Point::new(line, Column(7))));
        assert_eq!(url(&term, 0, 0), None);
    }

    #[test]
    fn osc8_hyperlink_with_another_scheme_is_ignored() {
        let term = term("\x1b]8;;file:///etc/passwd\x1b\\label\x1b]8;;\x1b\\", 40, 2);
        assert_eq!(url(&term, 0, 1), None);
    }

    #[test]
    fn a_url_is_looked_for_only_near_the_pointer_on_a_line_that_fills_the_scrollback() {
        // One logical line: a URL, then 300 rows of the same token, as minified output prints.
        let text = format!("https://x.test/{}", "a".repeat(40 * 300));
        let term = term(&text, 40, 10);
        let last_row = grid_point(&term, Point::new(9, Column(0)));
        let start_row = term.line_search_left(last_row).line;
        assert_eq!(last_row.line - start_row, 300);
        let mut regex = RegexSearch::new(URL_PATTERN).unwrap();
        let close = Point::new(start_row + URL_ROWS, Column(5));
        // Within reach, the URL is found from the rest of its line...
        assert!(url_at(&term, close, &mut regex).is_some());
        // ...but not from further down, where it would cost a pass over the whole line.
        let far = Point::new(start_row + URL_ROWS + 1, Column(5));
        assert_eq!(url_at(&term, far, &mut regex), None);
    }

    #[test]
    fn link_follows_scrollback() {
        let text = format!("https://old.test{}", "\r\n".repeat(5));
        let mut term = term(&text, 40, 3);
        assert_eq!(url(&term, 0, 0), None);
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(3));
        assert_eq!(url(&term, 0, 0).as_deref(), Some("https://old.test"));
    }

    #[test]
    fn trims_sentence_punctuation_and_unbalanced_brackets() {
        assert_eq!(trim_url("https://a.b/c."), "https://a.b/c");
        assert_eq!(trim_url("https://a.b/c)."), "https://a.b/c");
        assert_eq!(trim_url("https://a.b/f(x)"), "https://a.b/f(x)");
        assert_eq!(trim_url("https://a.b/[1]"), "https://a.b/[1]");
        assert_eq!(trim_url("https://a.b/c]';"), "https://a.b/c");
        assert_eq!(trim_url(""), "");
    }

    #[test]
    fn web_urls_need_a_scheme_and_a_host() {
        assert!(is_web_url("https://a"));
        assert!(is_web_url("HTTP://a"));
        assert!(!is_web_url("https://"));
        assert!(!is_web_url("http:/a"));
        assert!(!is_web_url("mailto:a@b"));
        assert!(!is_web_url("ht"));
    }

    #[test]
    fn url_pattern_compiles() {
        assert!(RegexSearch::new(URL_PATTERN).is_ok());
    }
}
