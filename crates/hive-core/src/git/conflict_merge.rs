//! Three-way merge document model parsed from Git conflict markers in a working file.
use similar::{Algorithm, ChangeTag, TextDiff};
use std::ops::Range;

/// Minimum proportion of shared tokens required before highlighting word-level diffs.
const MIN_INLINE_SIMILARITY: f32 = 0.25;
/// Skip inline word diffing on unusually long lines to keep rendering responsive.
const INLINE_MAX_LINE_BYTES: usize = 1000;

/// Error returned when conflict markers in a working file do not pair up cleanly.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct MergeParseError {
    /// One-based line number where the malformed marker appeared.
    pub line: usize,
    /// Human-readable description of the marker mismatch.
    pub message: &'static str,
}

/// One source line from the working file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    /// One-based line number in the working file.
    pub number: usize,
    /// Exact line content including its trailing newline, if any.
    raw: String,
}

impl SourceLine {
    fn from_raw(number: usize, raw: &str) -> Self {
        Self {
            number,
            raw: raw.to_owned(),
        }
    }
    /// Display text without the line ending.
    pub fn text(&self) -> &str {
        self.raw
            .strip_suffix("\r\n")
            .or_else(|| self.raw.strip_suffix('\n'))
            .unwrap_or(&self.raw)
    }
}

/// How the user resolved one conflict region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep the current (left / ours) lines.
    Current,
    /// Keep the incoming (right / theirs) lines.
    Incoming,
    /// Keep current lines followed by incoming lines.
    BothCurrentFirst,
    /// Keep incoming lines followed by current lines.
    BothIncomingFirst,
    /// Custom text edited directly by the user.
    Custom(String),
}

impl ConflictResolution {
    /// Short label describing the chosen resolution.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Current => "Accepted Current",
            Self::Incoming => "Accepted Incoming",
            Self::BothCurrentFirst => "Accepted Both (Current + Incoming)",
            Self::BothIncomingFirst => "Accepted Both (Incoming + Current)",
            Self::Custom(_) => "Edited Result",
        }
    }
}

/// One parsed conflict block inside a working file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictRegion {
    /// Zero-based index among all conflicts in the file.
    pub index: usize,
    /// One-based working-file line range covering the markers and their content.
    pub lines: Range<usize>,
    /// Label from the `<<<<<<<` marker (for example `HEAD`).
    pub current_label: String,
    /// Label from the `|||||||` marker when `diff3` / `zdiff3` style was used.
    pub base_label: Option<String>,
    /// Label from the `>>>>>>>` marker (for example `develop`).
    pub incoming_label: String,
    /// Lines from the current side.
    pub current: Vec<SourceLine>,
    /// Common-ancestor lines when `diff3` / `zdiff3` markers were present.
    pub base: Option<Vec<SourceLine>>,
    /// Lines from the incoming side.
    pub incoming: Vec<SourceLine>,
}

/// A contiguous segment of a working file with conflict markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeSegment {
    /// Shared lines outside any conflict markers.
    Common(Vec<SourceLine>),
    /// A conflict block with current, optional base, and incoming sides.
    Conflict(ConflictRegion),
}

/// Parsed working file split into shared segments and conflict regions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeDocument {
    /// Ordered segments covering every non-marker line of the file.
    pub segments: Vec<MergeSegment>,
    /// Total number of conflict regions.
    pub conflict_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParsePart {
    Current,
    Base,
    Incoming,
}

struct OpenConflict {
    part: ParsePart,
    marker_width: usize,
    region: ConflictRegion,
}

impl MergeDocument {
    /// Parses a working file containing Git conflict markers (2-way or `diff3`/`zdiff3`).
    pub fn parse(text: &str) -> Result<Self, MergeParseError> {
        let mut segments = Vec::new();
        let mut common: Vec<SourceLine> = Vec::new();
        let mut open: Option<OpenConflict> = None;
        let mut conflict_count = 0usize;

        for (ix, raw) in text.split_inclusive('\n').enumerate() {
            let line_no = ix + 1;
            let trimmed = raw.trim_end_matches(['\n', '\r']);
            // Git can widen the outer markers when either side already contains marker text.
            let token = trimmed.split_once(' ').map_or(trimmed, |(word, _)| word);
            let width = open
                .as_ref()
                .map_or(token.len().max(7), |state| state.marker_width);
            let marker = |byte| {
                (token.len() == width && token.bytes().all(|ch| ch == byte))
                    .then(|| trimmed[token.len()..].trim())
            };
            let (opening, base, separator, closing) =
                (marker(b'<'), marker(b'|'), marker(b'='), marker(b'>'));

            match open.as_mut() {
                None => {
                    if let Some(label) = opening {
                        if !common.is_empty() {
                            segments.push(MergeSegment::Common(std::mem::take(&mut common)));
                        }
                        open = Some(OpenConflict {
                            part: ParsePart::Current,
                            marker_width: width,
                            region: ConflictRegion {
                                index: conflict_count,
                                lines: line_no..line_no,
                                current_label: label.to_owned(),
                                base_label: None,
                                incoming_label: String::new(),
                                current: Vec::new(),
                                base: None,
                                incoming: Vec::new(),
                            },
                        });
                        continue;
                    }
                    if base.is_some() || closing.is_some() {
                        return Err(MergeParseError {
                            line: line_no,
                            message: "conflict marker outside a conflict",
                        });
                    }
                    common.push(SourceLine::from_raw(line_no, raw));
                }
                Some(state) => {
                    if opening.is_some() {
                        return Err(MergeParseError {
                            line: line_no,
                            message: "nested conflict marker",
                        });
                    }
                    match (state.part, base, separator, closing) {
                        (ParsePart::Current, Some(label), _, _) => {
                            state.region.base_label = Some(label.to_owned());
                            state.region.base = Some(Vec::new());
                            state.part = ParsePart::Base;
                            continue;
                        }
                        (ParsePart::Current | ParsePart::Base, _, Some(_), _) => {
                            state.part = ParsePart::Incoming;
                            continue;
                        }
                        (ParsePart::Incoming, _, _, Some(label)) => {
                            state.region.incoming_label = label.to_owned();
                            state.region.lines.end = line_no + 1;
                            if let Some(finished) = open.take() {
                                segments.push(MergeSegment::Conflict(finished.region));
                                conflict_count += 1;
                            }
                            continue;
                        }
                        (_, Some(_), _, _) | (_, _, Some(_), _) | (_, _, _, Some(_)) => {
                            return Err(MergeParseError {
                                line: line_no,
                                message: "unexpected marker in conflict",
                            });
                        }
                        _ => {}
                    }
                    let line = SourceLine::from_raw(line_no, raw);
                    match state.part {
                        ParsePart::Current => state.region.current.push(line),
                        ParsePart::Base => {
                            if let Some(base_lines) = state.region.base.as_mut() {
                                base_lines.push(line);
                            }
                        }
                        ParsePart::Incoming => state.region.incoming.push(line),
                    }
                }
            }
        }

        if open.is_some() {
            return Err(MergeParseError {
                line: text.lines().count().max(1),
                message: "unterminated conflict",
            });
        }
        if !common.is_empty() {
            segments.push(MergeSegment::Common(common));
        }

        Ok(Self {
            segments,
            conflict_count,
        })
    }

    /// Returns the conflict region at `index`, if any.
    pub fn conflict(&self, index: usize) -> Option<&ConflictRegion> {
        self.segments.iter().find_map(|segment| match segment {
            MergeSegment::Conflict(region) if region.index == index => Some(region),
            _ => None,
        })
    }

    /// Iterates over every conflict region in source order.
    pub fn conflicts(&self) -> impl Iterator<Item = &ConflictRegion> {
        self.segments.iter().filter_map(|segment| match segment {
            MergeSegment::Conflict(region) => Some(region),
            _ => None,
        })
    }

    /// Assembles the full merged file text once every conflict has a resolution.
    ///
    /// Returns `None` when the file had no conflict markers (nothing needs rewriting; stage as-is)
    /// or when at least one conflict is still undecided.
    pub fn resolved_text(&self, resolutions: &[Option<ConflictResolution>]) -> Option<String> {
        if self.conflict_count == 0 || resolutions.len() != self.conflict_count {
            return None;
        }
        let mut out = String::new();
        for (ix, segment) in self.segments.iter().enumerate() {
            let has_following = ix + 1 < self.segments.len();
            match segment {
                MergeSegment::Common(lines) => {
                    for line in lines {
                        out.push_str(&line.raw);
                    }
                }
                MergeSegment::Conflict(region) => {
                    let resolution = resolutions.get(region.index)?.as_ref()?;
                    let chunk = region.resolution_text(resolution);
                    out.push_str(&chunk);
                    if has_following && !chunk.is_empty() && !chunk.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
        }
        Some(out)
    }
}

impl ConflictRegion {
    /// Text contributed by this region, in the selected order, with original line endings.
    pub fn resolution_text(&self, resolution: &ConflictResolution) -> String {
        let (first, second): (&[SourceLine], &[SourceLine]) = match resolution {
            ConflictResolution::Current => (&self.current, &[]),
            ConflictResolution::Incoming => (&self.incoming, &[]),
            ConflictResolution::BothCurrentFirst => (&self.current, &self.incoming),
            ConflictResolution::BothIncomingFirst => (&self.incoming, &self.current),
            ConflictResolution::Custom(text) => return text.clone(),
        };
        first
            .iter()
            .chain(second)
            .map(|line| line.raw.as_str())
            .collect()
    }
}

/// Word-level changed byte ranges on each line of a conflict's Current and Incoming sides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineDiffRanges {
    /// Per-line changed byte ranges for the `current` lines.
    pub current: Vec<Vec<Range<usize>>>,
    /// Per-line changed byte ranges for the `incoming` lines.
    pub incoming: Vec<Vec<Range<usize>>>,
}

/// Computes word-level inline diff byte ranges for `current` vs `incoming` lines in one conflict.
pub fn inline_diff_ranges(current: &[SourceLine], incoming: &[SourceLine]) -> InlineDiffRanges {
    let mut current_ranges = vec![Vec::new(); current.len()];
    let mut incoming_ranges = vec![Vec::new(); incoming.len()];
    if current.is_empty() || incoming.is_empty() {
        return InlineDiffRanges {
            current: current_ranges,
            incoming: incoming_ranges,
        };
    }

    let left_text: Vec<&str> = current.iter().map(|line| line.text()).collect();
    let right_text: Vec<&str> = incoming.iter().map(|line| line.text()).collect();
    let line_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(std::time::Duration::from_millis(100))
        .diff_slices(&left_text, &right_text);

    for op in line_diff.ops() {
        if let similar::DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } = *op
        {
            let paired = old_len.min(new_len);
            for offset in 0..paired {
                let left = current[old_index + offset].text();
                let right = incoming[new_index + offset].text();
                let (l_spans, r_spans) = word_diff_pair(left, right);
                current_ranges[old_index + offset] = l_spans;
                incoming_ranges[new_index + offset] = r_spans;
            }
        }
    }
    InlineDiffRanges {
        current: current_ranges,
        incoming: incoming_ranges,
    }
}

fn word_diff_pair(left: &str, right: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if left.len() > INLINE_MAX_LINE_BYTES
        || right.len() > INLINE_MAX_LINE_BYTES
        || left == right
        || left.is_empty()
        || right.is_empty()
    {
        return (Vec::new(), Vec::new());
    }
    let left_tokens = tokenize_words(left);
    let right_tokens = tokenize_words(right);
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&left_tokens, &right_tokens);
    if diff.ratio() < MIN_INLINE_SIMILARITY {
        return (Vec::new(), Vec::new());
    }

    let mut left_spans = Vec::new();
    let mut right_spans = Vec::new();
    let (mut left_pos, mut right_pos) = (0usize, 0usize);
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                left_pos += len;
                right_pos += len;
            }
            ChangeTag::Delete => {
                push_merged(&mut left_spans, left_pos..left_pos + len);
                left_pos += len;
            }
            ChangeTag::Insert => {
                push_merged(&mut right_spans, right_pos..right_pos + len);
                right_pos += len;
            }
        }
    }
    (left_spans, right_spans)
}

/// Splits `text` into contiguous identifier/number words, whitespace runs, and single
/// punctuation characters so word-level inline diffs isolate changed identifiers cleanly.
fn tokenize_words(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut iter = text.char_indices().peekable();
    while let Some((start, ch)) = iter.next() {
        let is_word = ch.is_alphanumeric() || ch == '_';
        let is_space = ch.is_whitespace();
        let mut end = start + ch.len_utf8();
        if is_word || is_space {
            while let Some(&(next_ix, next_ch)) = iter.peek() {
                let same_kind = if is_word {
                    next_ch.is_alphanumeric() || next_ch == '_'
                } else {
                    next_ch.is_whitespace()
                };
                if !same_kind {
                    break;
                }
                end = next_ix + next_ch.len_utf8();
                iter.next();
            }
        }
        tokens.push(&text[start..end]);
    }
    tokens
}

fn push_merged(spans: &mut Vec<Range<usize>>, next: Range<usize>) {
    if next.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.end == next.start
    {
        last.end = next.end;
        return;
    }
    spans.push(next);
}

/// Returns the sub-range of a `Common` segment of `len` lines that can be collapsed
/// when `context` lines are kept next to each bordering conflict.
pub fn collapsed_common_range(
    len: usize,
    has_conflict_above: bool,
    has_conflict_below: bool,
    context: usize,
    min_collapsed: usize,
) -> Option<Range<usize>> {
    let start = if has_conflict_above {
        context.min(len)
    } else {
        0
    };
    let end = if has_conflict_below {
        len.saturating_sub(context)
    } else {
        len
    };
    if end > start && end - start >= min_collapsed {
        Some(start..end)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_WAY: &str = "import a;\n<<<<<<< HEAD\nconst x = 1;\n=======\nconst x = 2;\nconst y = 3;\n>>>>>>> develop\nexport { x };\n";
    const DIFF3: &str =
        "top\n<<<<<<< HEAD\nours\n||||||| base\noriginal\n=======\ntheirs\n>>>>>>> feature\nend\n";

    #[test]
    fn parses_two_way_and_diff3_conflicts_with_line_numbers() {
        let doc = MergeDocument::parse(TWO_WAY).unwrap();
        assert_eq!(doc.conflict_count, 1);
        assert_eq!(doc.segments.len(), 3);
        let MergeSegment::Common(top) = &doc.segments[0] else {
            panic!("expected top common");
        };
        assert_eq!(top.len(), 1);
        assert_eq!((top[0].number, top[0].text()), (1, "import a;"));

        let conflict = doc.conflict(0).unwrap();
        assert_eq!(conflict.lines, 2..8);
        assert_eq!(conflict.current_label, "HEAD");
        assert_eq!(conflict.incoming_label, "develop");
        assert_eq!(conflict.base_label, None);
        assert_eq!(conflict.current.len(), 1);
        assert_eq!(conflict.current[0].number, 3);
        assert_eq!(conflict.incoming.len(), 2);
        assert_eq!(conflict.incoming[0].number, 5);
        assert_eq!(conflict.incoming[1].number, 6);

        let diff3 = MergeDocument::parse(DIFF3).unwrap();
        let region = diff3.conflict(0).unwrap();
        assert_eq!(region.base_label.as_deref(), Some("base"));
        let base = region.base.as_ref().unwrap();
        assert_eq!(base.len(), 1);
        assert_eq!(base[0].text(), "original");
        assert_eq!(base[0].number, 5);
    }

    #[test]
    fn wide_markers_keep_literal_shorter_markers_inside_each_side() {
        let doc = MergeDocument::parse(
            "<<<<<<<<< HEAD\n<<<<<<< sample\n=========\ntheirs\n>>>>>>>>> topic\n",
        )
        .unwrap();
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::Current)])
                .as_deref(),
            Some("<<<<<<< sample\n")
        );
    }

    #[test]
    fn rejects_malformed_conflict_markers() {
        for broken in [
            "<<<<<<< HEAD\nours\n",
            "<<<<<<< HEAD\n<<<<<<< nested\n",
            "<<<<<<< HEAD\na\n=======\nb\n=======\nc\n>>>>>>> incoming\n",
            "<<<<<<< HEAD\na\n>>>>>>> out of order\n=======\nb\n>>>>>>> incoming\n",
            ">>>>>>> stray\n",
            "||||||| stray\n",
        ] {
            let err = MergeDocument::parse(broken).unwrap_err();
            assert!(err.line > 0, "{broken:?}");
        }
        // A separator outside a conflict is ordinary text (e.g. Markdown Setext heading).
        let md = MergeDocument::parse("Title\n=======\nBody\n").unwrap();
        assert_eq!(md.conflict_count, 0);
    }

    #[test]
    fn resolved_text_requires_all_conflicts_and_supports_all_variants() {
        let doc = MergeDocument::parse(TWO_WAY).unwrap();
        assert_eq!(doc.resolved_text(&[None]), None);
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::Current)])
                .unwrap(),
            "import a;\nconst x = 1;\nexport { x };\n"
        );
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::Incoming)])
                .unwrap(),
            "import a;\nconst x = 2;\nconst y = 3;\nexport { x };\n"
        );
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::BothCurrentFirst)])
                .unwrap(),
            "import a;\nconst x = 1;\nconst x = 2;\nconst y = 3;\nexport { x };\n"
        );
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::BothIncomingFirst)])
                .unwrap(),
            "import a;\nconst x = 2;\nconst y = 3;\nconst x = 1;\nexport { x };\n"
        );
        // Custom text without trailing newline inserts separator before following segment.
        assert_eq!(
            doc.resolved_text(&[Some(ConflictResolution::Custom("const x = 42;".into()))])
                .unwrap(),
            "import a;\nconst x = 42;\nexport { x };\n"
        );
        // Marker-free file has no conflict edits to apply.
        let clean = MergeDocument::parse("plain\n").unwrap();
        assert_eq!(clean.conflict_count, 0);
        assert_eq!(clean.resolved_text(&[]), None);
    }

    #[test]
    fn inline_diff_highlights_changed_words_between_sides() {
        let cur = [SourceLine::from_raw(
            1,
            "const snap = getSnapClickId(foo);\n",
        )];
        let inc = [SourceLine::from_raw(
            1,
            "const snap = getSnapEventId(foo);\n",
        )];
        let diffs = inline_diff_ranges(&cur, &inc);
        assert_eq!(
            &cur[0].text()[diffs.current[0][0].clone()],
            "getSnapClickId"
        );
        assert_eq!(
            &inc[0].text()[diffs.incoming[0][0].clone()],
            "getSnapEventId"
        );
    }

    #[test]
    fn collapsed_common_range_preserves_context_around_conflicts() {
        assert_eq!(collapsed_common_range(10, true, true, 3, 2), Some(3..7));
        assert_eq!(collapsed_common_range(7, true, true, 3, 2), None);
        assert_eq!(collapsed_common_range(10, false, true, 3, 2), Some(0..7));
        assert_eq!(collapsed_common_range(10, true, false, 3, 2), Some(3..10));
    }
}
