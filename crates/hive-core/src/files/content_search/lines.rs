//! Turn regex matches in one document into display lines. Pure; no I/O.
use super::LineMatch;
use regex::Regex;
use std::ops::Range;

/// Longest preview; wider than any sidebar, and it bounds minified one-line files.
const PREVIEW_MAX_BYTES: usize = 240;
/// Context kept before a match that starts far into its line, so it stays visible.
const PREVIEW_LEAD_BYTES: usize = 24;
const ELLIPSIS: &str = "…";

/// The line being assembled while matches on it keep arriving.
struct Line {
    number: u32,
    start: usize,
    /// End of the line's content, excluding `\n` and a trailing `\r`.
    end: usize,
    ranges: Vec<Range<usize>>,
}

impl Line {
    /// Keep the part of a match that lies on this line; a multi-line match is cut at its end.
    fn add(&mut self, range: Range<usize>) {
        let end = range.end.min(self.end);
        if range.start < end {
            self.ranges.push(range.start - self.start..end - self.start);
        }
    }

    fn finish(self, text: &str) -> Option<LineMatch> {
        let first = self.ranges.first()?.start;
        let line = &text[self.start..self.end];
        let (preview, ranges) = preview(line, &self.ranges);
        Some(LineMatch {
            line: self.number,
            column: u32::try_from(line[..first].chars().count()).unwrap_or(u32::MAX),
            preview,
            ranges,
        })
    }
}

/// Collect up to `limit` matching lines in one pass over `text`. Line numbers are counted
/// incrementally between matches, so a large file costs one scan, not one per line.
pub(super) fn matches(text: &str, regex: &Regex, limit: usize) -> Vec<LineMatch> {
    let mut result = Vec::new();
    let mut current: Option<Line> = None;
    let (mut counted, mut number) = (0, 0u32);
    for found in regex.find_iter(text) {
        if found.is_empty() {
            continue;
        }
        if let Some(line) = &mut current
            && found.start() < line.end
        {
            line.add(found.range());
            continue;
        }
        if let Some(line) = current.take().and_then(|line| line.finish(text)) {
            result.push(line);
            if result.len() == limit {
                return result;
            }
        }
        let start = text[counted..found.start()]
            .rfind('\n')
            .map_or(counted, |ix| counted + ix + 1);
        let newlines = text[counted..start].matches('\n').count();
        number = number.saturating_add(u32::try_from(newlines).unwrap_or(u32::MAX));
        counted = start;
        let end = text[start..].find('\n').map_or(text.len(), |ix| start + ix);
        let end = if text[..end].ends_with('\r') {
            end - 1
        } else {
            end
        };
        let mut line = Line {
            number,
            start,
            end,
            ranges: Vec::new(),
        };
        line.add(found.range());
        current = Some(line);
    }
    if let Some(line) = current.and_then(|line| line.finish(text))
        && result.len() < limit
    {
        result.push(line);
    }
    result
}

/// Trim indentation (never past the first match), cut long leading context to a short lead,
/// cap the length, and shift `ranges` into the returned text.
fn preview(line: &str, ranges: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let first = ranges.first().map_or(0, |range| range.start);
    let indent = line.len() - line.trim_start().len();
    let mut from = indent.min(first);
    let mut prefix = "";
    if first - from > PREVIEW_LEAD_BYTES {
        from = line.floor_char_boundary(first - PREVIEW_LEAD_BYTES);
        prefix = ELLIPSIS;
    }
    let mut to = line.len();
    let mut suffix = "";
    if to - from > PREVIEW_MAX_BYTES {
        to = line.floor_char_boundary(from + PREVIEW_MAX_BYTES);
        suffix = ELLIPSIS;
    }
    let shift = prefix.len();
    let shifted = ranges
        .iter()
        .filter_map(|range| {
            let (start, end) = (range.start.max(from), range.end.min(to));
            (start < end).then(|| start - from + shift..end - from + shift)
        })
        .collect();
    (format!("{prefix}{}{suffix}", &line[from..to]), shifted)
}
