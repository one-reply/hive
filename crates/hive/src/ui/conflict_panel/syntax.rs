//! Syntax highlighting and inline word-diff presentation for the 3-pane conflict view.
use gpui_kit::component::Rope;
use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_kit::{HighlightStyle, Hsla};
use hive_core::files::editor_language;
use hive_core::git::conflict_merge::{
    ConflictResolution, InlineDiffRanges, MergeDocument, MergeSegment, inline_diff_ranges,
};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

type LineStyles = Vec<(Range<usize>, HighlightStyle)>;

/// Prepared per-line syntax styles and inline word diffs for one `MergeDocument` and theme.
#[derive(Clone, Default)]
pub(super) struct MergeHighlights {
    /// `(segment_ix, line_ix)` -> highlight styles for shared lines.
    common: HashMap<(usize, usize), LineStyles>,
    /// Per-conflict `(current, base, incoming)` highlight styles.
    conflicts: Vec<ConflictSideHighlights>,
    /// Per-conflict inline word-diff byte ranges.
    inline: Vec<InlineDiffRanges>,
    /// Language used to highlight custom-edited conflict snippets on demand.
    language: Option<String>,
}

#[derive(Clone, Default)]
struct ConflictSideHighlights {
    current: Vec<LineStyles>,
    base: Vec<LineStyles>,
    incoming: Vec<LineStyles>,
}

impl MergeHighlights {
    /// Blocking: prepares syntax styles for Current and Incoming panes plus word-level diffs.
    pub(super) fn prepare(path: &str, doc: &MergeDocument, theme: &Arc<HighlightTheme>) -> Self {
        let language = editor_language(Path::new(path));
        let mut out = Self {
            common: HashMap::new(),
            conflicts: vec![ConflictSideHighlights::default(); doc.conflict_count],
            inline: Vec::with_capacity(doc.conflict_count),
            language: (language != "text").then_some(language),
        };

        for segment in &doc.segments {
            if let MergeSegment::Conflict(region) = segment {
                out.inline
                    .push(inline_diff_ranges(&region.current, &region.incoming));
            }
        }

        let Some(language) = out.language.as_deref() else {
            return out;
        };

        let current_spans = highlight_side(doc, language, SideTarget::Current, theme);
        let incoming_spans = highlight_side(doc, language, SideTarget::Incoming, theme);
        let base_spans = highlight_base(doc, language, theme);

        for (seg_ix, segment) in doc.segments.iter().enumerate() {
            match segment {
                MergeSegment::Common(lines) => {
                    for line_ix in 0..lines.len() {
                        if let Some(spans) = current_spans.common.get(&(seg_ix, line_ix)) {
                            out.common.insert((seg_ix, line_ix), spans.clone());
                        }
                    }
                }
                MergeSegment::Conflict(region) => {
                    if let Some(spans) = current_spans.conflicts.get(region.index) {
                        out.conflicts[region.index].current = spans.clone();
                    }
                    if let Some(spans) = incoming_spans.conflicts.get(region.index) {
                        out.conflicts[region.index].incoming = spans.clone();
                    }
                    if let Some(spans) = base_spans.get(&region.index) {
                        out.conflicts[region.index].base = spans.clone();
                    }
                }
            }
        }
        out
    }

    pub(super) fn common_styles(&self, segment: usize, line: usize) -> LineStyles {
        self.common
            .get(&(segment, line))
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn current_styles(
        &self,
        conflict: usize,
        line: usize,
        inline_bg: Hsla,
    ) -> LineStyles {
        let syntax = self
            .conflicts
            .get(conflict)
            .and_then(|c| c.current.get(line));
        let words = self
            .inline
            .get(conflict)
            .and_then(|diff| diff.current.get(line));
        word_styles(syntax, words, inline_bg)
    }

    pub(super) fn incoming_styles(
        &self,
        conflict: usize,
        line: usize,
        inline_bg: Hsla,
    ) -> LineStyles {
        let syntax = self
            .conflicts
            .get(conflict)
            .and_then(|c| c.incoming.get(line));
        let words = self
            .inline
            .get(conflict)
            .and_then(|diff| diff.incoming.get(line));
        word_styles(syntax, words, inline_bg)
    }

    pub(super) fn base_styles(&self, conflict: usize, line: usize) -> LineStyles {
        self.conflicts
            .get(conflict)
            .and_then(|c| c.base.get(line))
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn result_styles(
        &self,
        region: &hive_core::git::conflict_merge::ConflictRegion,
        conflict: usize,
        resolution: &ConflictResolution,
        line: usize,
        text: &str,
        theme: &HighlightTheme,
    ) -> LineStyles {
        let Some(side) = self.conflicts.get(conflict) else {
            return Vec::new();
        };
        match resolution {
            ConflictResolution::Current => side.current.get(line).cloned().unwrap_or_default(),
            ConflictResolution::Incoming => side.incoming.get(line).cloned().unwrap_or_default(),
            ConflictResolution::BothCurrentFirst => {
                if line < region.current.len() {
                    side.current.get(line).cloned().unwrap_or_default()
                } else {
                    side.incoming
                        .get(line - region.current.len())
                        .cloned()
                        .unwrap_or_default()
                }
            }
            ConflictResolution::BothIncomingFirst => {
                if line < region.incoming.len() {
                    side.incoming.get(line).cloned().unwrap_or_default()
                } else {
                    side.current
                        .get(line - region.incoming.len())
                        .cloned()
                        .unwrap_or_default()
                }
            }
            ConflictResolution::Custom(_) => {
                let Some(language) = self.language.as_deref() else {
                    return Vec::new();
                };
                highlight_single_line(language, text, theme)
            }
        }
    }
}

fn word_styles(
    syntax: Option<&LineStyles>,
    words: Option<&Vec<Range<usize>>>,
    background: Hsla,
) -> LineStyles {
    let Some(words) = words.filter(|ranges| !ranges.is_empty()) else {
        return syntax.cloned().unwrap_or_default();
    };
    gpui_kit::combine_highlights(
        syntax.into_iter().flatten().cloned(),
        words.iter().cloned().map(|range| {
            (
                range,
                HighlightStyle {
                    background_color: Some(background),
                    ..Default::default()
                },
            )
        }),
    )
    .collect()
}

#[derive(Clone, Copy)]
enum SideTarget {
    Current,
    Incoming,
}

struct SideCaptures {
    common: HashMap<(usize, usize), LineStyles>,
    conflicts: Vec<Vec<LineStyles>>,
}

fn highlight_side(
    doc: &MergeDocument,
    language: &str,
    target: SideTarget,
    theme: &HighlightTheme,
) -> SideCaptures {
    enum Slot {
        Common { segment: usize, line: usize },
        Conflict { conflict: usize, line: usize },
    }
    let mut source = String::new();
    let mut slots: Vec<(Range<usize>, Slot)> = Vec::new();
    for (seg_ix, segment) in doc.segments.iter().enumerate() {
        match segment {
            MergeSegment::Common(lines) => {
                for (line_ix, line) in lines.iter().enumerate() {
                    let start = source.len();
                    source.push_str(line.text());
                    let end = source.len();
                    source.push('\n');
                    slots.push((
                        start..end,
                        Slot::Common {
                            segment: seg_ix,
                            line: line_ix,
                        },
                    ));
                }
            }
            MergeSegment::Conflict(region) => {
                let lines = match target {
                    SideTarget::Current => &region.current,
                    SideTarget::Incoming => &region.incoming,
                };
                for (line_ix, line) in lines.iter().enumerate() {
                    let start = source.len();
                    source.push_str(line.text());
                    let end = source.len();
                    source.push('\n');
                    slots.push((
                        start..end,
                        Slot::Conflict {
                            conflict: region.index,
                            line: line_ix,
                        },
                    ));
                }
            }
        }
    }

    let mut common = HashMap::new();
    let mut conflicts: Vec<Vec<LineStyles>> = doc
        .conflicts()
        .map(|region| {
            let len = match target {
                SideTarget::Current => region.current.len(),
                SideTarget::Incoming => region.incoming.len(),
            };
            vec![Vec::new(); len]
        })
        .collect();
    if source.is_empty() {
        return SideCaptures { common, conflicts };
    }

    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from_str(&source), None);
    for (capture, style) in highlighter.styles(&(0..source.len()), theme) {
        let first = slots.partition_point(|(range, _)| range.end <= capture.start);
        for (line_range, slot) in &slots[first..] {
            if line_range.start >= capture.end {
                break;
            }
            let start = capture.start.max(line_range.start) - line_range.start;
            let end = capture.end.min(line_range.end) - line_range.start;
            if start < end {
                let entry = (start..end, style);
                match *slot {
                    Slot::Common { segment, line } => {
                        common.entry((segment, line)).or_default().push(entry);
                    }
                    Slot::Conflict { conflict, line } => {
                        if let Some(lines) = conflicts.get_mut(conflict)
                            && let Some(bucket) = lines.get_mut(line)
                        {
                            bucket.push(entry);
                        }
                    }
                }
            }
        }
    }

    SideCaptures { common, conflicts }
}

fn highlight_base(
    doc: &MergeDocument,
    language: &str,
    theme: &HighlightTheme,
) -> HashMap<usize, Vec<LineStyles>> {
    let mut map = HashMap::new();
    let mut highlighter = SyntaxHighlighter::new(language);
    for region in doc.conflicts() {
        let Some(base) = &region.base else {
            continue;
        };
        let mut per_line = Vec::with_capacity(base.len());
        for line in base {
            if line.text().is_empty() {
                per_line.push(Vec::new());
                continue;
            }
            highlighter.update(None, &Rope::from_str(line.text()), None);
            per_line.push(highlighter.styles(&(0..line.text().len()), theme));
        }
        map.insert(region.index, per_line);
    }
    map
}

fn highlight_single_line(language: &str, text: &str, theme: &HighlightTheme) -> LineStyles {
    if text.is_empty() {
        return Vec::new();
    }
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from_str(text), None);
    highlighter.styles(&(0..text.len()), theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_and_syntax_styles_share_disjoint_utf8_runs() {
        let syntax = vec![(
            0..8,
            HighlightStyle {
                color: Some(gpui_kit::red()),
                ..Default::default()
            },
        )];
        let ranges = vec![2..4, 6..8];
        let result = word_styles(Some(&syntax), Some(&ranges), gpui_kit::blue());
        assert_eq!(
            result.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            [0..2, 2..4, 4..6, 6..8]
        );
        assert!(result.iter().all(|(_, style)| style.color.is_some()));
        assert_eq!(result[1].1.background_color, Some(gpui_kit::blue()));
    }
}
