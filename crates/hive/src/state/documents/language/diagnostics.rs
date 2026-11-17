use super::*;
use gpui_kit::component::input::RopeExt as _;

/// Lines one diagnostic may span and still be compared across document versions.
const CARRIED_LINES: u32 = 200;

/// One server publication and, when it was computed for the synced buffer, that buffer.
pub(crate) struct Publication {
    params: PublishDiagnosticsParams,
    /// The document as the server saw it for `params.version`.
    diagnosed: Option<Rope>,
}

impl LanguageSession {
    pub(super) fn receive_diagnostics(&mut self, params: PublishDiagnosticsParams) {
        let diagnosed = self
            .documents
            .get(&params.uri)
            .filter(|synced| params.version == Some(synced.version))
            .map(|synced| synced.text.clone());
        self.diagnostics_limited |= retain_bounded(&mut self.diagnostics, params, diagnosed);
    }
    pub(super) fn current_diagnostics(&self, uri: &Uri, text: &Rope) -> Vec<lsp_types::Diagnostic> {
        self.documents
            .get(uri)
            .zip(self.diagnostics.get(uri.as_str()))
            .map(|(synced, publication)| visible(synced, text, publication))
            .unwrap_or_default()
    }
}

/// The diagnostics of `publication` that describe `text`, the buffer the editor shows.
fn visible(synced: &Synced, text: &Rope, publication: &Publication) -> Vec<lsp_types::Diagnostic> {
    if synced.text != *text {
        return Vec::new();
    }
    let diagnostics = &publication.params.diagnostics;
    match (publication.params.version, &publication.diagnosed) {
        (None, _) => diagnostics.clone(),
        (Some(version), _) if version == synced.version => diagnostics.clone(),
        // rust-analyzer does not republish a set that an edit left identical, so an older
        // publication still holds wherever the edit neither changed nor moved the lines.
        (Some(_), Some(diagnosed)) => diagnostics
            .iter()
            .filter(|diagnostic| same_lines(diagnosed, text, diagnostic.range))
            .cloned()
            .collect(),
        (Some(_), None) => Vec::new(),
    }
}

fn same_lines(diagnosed: &Rope, text: &Rope, range: lsp_types::Range) -> bool {
    let (start, end) = (range.start.line, range.end.line);
    start <= end
        && end - start < CARRIED_LINES
        && (start..=end).all(|line| {
            let line = line as usize;
            line < diagnosed.lines_len()
                && line < text.lines_len()
                && diagnosed.slice_line(line) == text.slice_line(line)
        })
}

/// Editor ranges in ascending order: the editor's diagnostic set seeks by position and loses
/// entries pushed out of order, and servers publish in no particular order.
pub(super) fn for_editor(
    source: &str,
    diagnostics: &[lsp_types::Diagnostic],
) -> Vec<lsp_types::Diagnostic> {
    let lines = position::Lines::new(source);
    let mut converted: Vec<_> = diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let mut diagnostic = diagnostic.clone();
            diagnostic.range = lines.editor_range(diagnostic.range)?;
            Some(diagnostic)
        })
        .collect();
    converted.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
    converted
}

impl Documents {
    pub(crate) fn problems_limited(&self, target: &Target, cx: &App) -> bool {
        self.problem_sessions(target)
            .iter()
            .any(|session| session.read(cx).diagnostics_limited)
    }

    pub(crate) fn problem_sessions(&self, target: &Target) -> Vec<Entity<LanguageSession>> {
        self.language_sessions
            .iter()
            .filter(|(host, root, _, _)| *host == target.host && *root == target.path)
            .filter_map(|(_, _, _, session)| session.upgrade())
            .collect()
    }
    pub(crate) fn problems(
        &self,
        target: &Target,
        cx: &App,
    ) -> Vec<hive_core::files::excerpts::Hit> {
        self.language_sessions
            .iter()
            .filter(|(host, root, _, _)| *host == target.host && *root == target.path)
            .filter_map(|(_, _, _, session)| session.upgrade())
            .flat_map(|session| {
                let session = session.read(cx);
                session
                    .diagnostics
                    .values()
                    .flat_map(|publication| {
                        let params = &publication.params;
                        let path = position::path(&params.uri);
                        let valid = self
                            .buffer(
                                &target.host,
                                path.as_deref().unwrap_or(std::path::Path::new("")),
                                cx,
                            )
                            .map(|text| session.current_diagnostics(&params.uri, &Rope::from(text)))
                            .unwrap_or_else(|| params.diagnostics.clone());
                        valid.into_iter().filter_map(move |diagnostic| {
                            Some(hive_core::files::excerpts::Hit {
                                path: path.clone()?,
                                line: diagnostic.range.start.line,
                                message: format!(
                                    "{}: {}",
                                    match diagnostic.severity {
                                        Some(DiagnosticSeverity::ERROR) => "Error",
                                        Some(DiagnosticSeverity::WARNING) => "Warning",
                                        _ => "Info",
                                    },
                                    diagnostic.message
                                ),
                            })
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

// Bound retained server data as well as individual wire frames; auxiliary edits/data are unused.
fn retain_bounded(
    map: &mut HashMap<String, Publication>,
    mut params: PublishDiagnosticsParams,
    diagnosed: Option<Rope>,
) -> bool {
    const LIMIT: usize = 5_000;
    map.remove(params.uri.as_str());
    if params.diagnostics.is_empty() {
        return false;
    }
    if params.uri.as_str().len() > 8192 || position::path(&params.uri).is_none() {
        return true;
    }
    let retained: usize = map.values().map(|p| p.params.diagnostics.len()).sum();
    let available = LIMIT.saturating_sub(retained);
    let limited = params.diagnostics.len() > available;
    params.diagnostics.truncate(available);
    for diagnostic in &mut params.diagnostics {
        diagnostic.message = diagnostic.message.chars().take(2048).collect();
        diagnostic.data = None;
        diagnostic.related_information = None;
        diagnostic.code_description = None;
        diagnostic.source = None;
        diagnostic.code = None;
    }
    if !params.diagnostics.is_empty() {
        map.insert(
            params.uri.as_str().into(),
            Publication { params, diagnosed },
        );
    }
    limited
}

#[cfg(test)]
mod tests {
    use super::{HashMap, Publication, Rope, Synced, for_editor, retain_bounded, visible};
    use lsp_types::*;

    fn at(line: u32, start: u32, end: u32) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, start), Position::new(line, end)),
            ..Diagnostic::default()
        }
    }
    fn publication(version: Option<i32>, diagnosed: Option<&Rope>) -> Publication {
        let uri: Uri = "file:///work/a.rs".parse().unwrap();
        Publication {
            params: PublishDiagnosticsParams::new(uri, vec![at(0, 0, 5), at(2, 0, 3)], version),
            diagnosed: diagnosed.cloned(),
        }
    }

    #[test]
    fn stale_diagnostic_versions_and_unsynchronized_edits_are_not_displayed() {
        let text = Rope::from("const café = '😀';\n\nlet");
        let synced = Synced {
            text: text.clone(),
            version: 3,
        };
        assert_eq!(
            visible(&synced, &text, &publication(Some(3), None)).len(),
            2
        );
        assert_eq!(visible(&synced, &text, &publication(None, None)).len(), 2);
        assert!(visible(&synced, &text, &publication(Some(2), None)).is_empty());
        assert!(visible(&synced, &text, &publication(Some(4), None)).is_empty());
        assert!(
            visible(
                &synced,
                &Rope::from("changed"),
                &publication(Some(3), Some(&text))
            )
            .is_empty()
        );
    }
    #[test]
    fn an_older_publication_holds_where_an_edit_left_the_lines_in_place() {
        let diagnosed = Rope::from("first\nsecond\nthird\n");
        let shown = |text: &str| {
            let text = Rope::from(text);
            let synced = Synced {
                text: text.clone(),
                version: 2,
            };
            visible(&synced, &text, &publication(Some(1), Some(&diagnosed)))
                .iter()
                .map(|diagnostic| diagnostic.range.start.line)
                .collect::<Vec<_>>()
        };
        // rust-analyzer stays silent after an edit below both diagnostics.
        assert_eq!(shown("first\nsecond\nthird\n// tail\n"), [0, 2]);
        // An edit on a diagnosed line, or one that moves it, hides only that diagnostic.
        assert_eq!(shown("first\nsecond\nthirds\n"), [0]);
        assert_eq!(shown("first\ninserted\nsecond\nthird\n"), [0]);
        assert!(shown("inserted\nfirst\nsecond\nthird\n").is_empty());
        // A line that no longer exists has nothing to mark.
        assert_eq!(shown("first"), [0]);
    }
    #[test]
    fn editor_diagnostics_are_sorted_by_position_whatever_the_server_order() {
        // The order rust-analyzer 1.99 published for one mismatched-types error.
        let server = [at(5, 24, 37), at(5, 15, 21), at(5, 38, 38), at(1, 0, 1)];
        let source = "a\nb\n\n\n\n    let wrong: String = util::total(3);;\n";
        let starts: Vec<_> = for_editor(source, &server)
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                )
            })
            .collect();
        assert_eq!(starts, [(1, 0), (5, 15), (5, 24), (5, 38)]);
        // A range outside the buffer is dropped, not clamped onto other text.
        assert!(for_editor("short", &[at(9, 0, 1)]).is_empty());
    }
    #[test]
    fn diagnostics_are_bounded_and_an_empty_publication_clears_the_file() {
        let uri: Uri = "file:///work/a.ts".parse().unwrap();
        let mut map = HashMap::new();
        assert!(retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(uri.clone(), vec![Diagnostic::default(); 5_001], Some(1)),
            None
        ));
        assert_eq!(map[uri.as_str()].params.diagnostics.len(), 5_000);
        let other: Uri = "file:///work/b.ts".parse().unwrap();
        assert!(retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(other.clone(), vec![Diagnostic::default()], None),
            None
        ));
        assert!(!map.contains_key(other.as_str()));
        assert!(!retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(uri, Vec::new(), Some(2)),
            None
        ));
        assert!(map.is_empty());
    }
}
