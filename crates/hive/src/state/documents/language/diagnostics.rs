use super::*;
use hive_core::language_service::markdown;

impl LanguageSession {
    pub(super) fn receive_diagnostics(&mut self, params: PublishDiagnosticsParams) {
        self.diagnostics_limited |= retain_bounded(&mut self.diagnostics, params);
    }
    pub(super) fn current_diagnostics(&self, uri: &Uri, text: &Rope) -> Vec<lsp_types::Diagnostic> {
        self.documents
            .get(uri)
            .zip(self.diagnostics.get(uri.as_str()))
            .filter(|(synced, params)| current(synced, text, params.version))
            .map(|(_, params)| params.diagnostics.clone())
            .unwrap_or_default()
    }
}

fn current(synced: &Synced, text: &Rope, version: Option<i32>) -> bool {
    synced.text == *text && version.is_none_or(|version| version == synced.version)
}

/// `diagnostics` as the editor shows them: editor positions, and messages restricted to web
/// links, since the popover renders a message as Markdown and opens any link in it.
pub(super) fn for_editor(
    source: &str,
    diagnostics: &[lsp_types::Diagnostic],
) -> Vec<lsp_types::Diagnostic> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let mut diagnostic = diagnostic.clone();
            diagnostic.range = position::editor_range(source, diagnostic.range)?;
            diagnostic.message = markdown::restrict_links(&diagnostic.message).into_owned();
            Some(diagnostic)
        })
        .collect()
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
                    .flat_map(|params| {
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
    map: &mut HashMap<String, PublishDiagnosticsParams>,
    mut params: PublishDiagnosticsParams,
) -> bool {
    const LIMIT: usize = 5_000;
    map.remove(params.uri.as_str());
    if params.diagnostics.is_empty() {
        return false;
    }
    if params.uri.as_str().len() > 8192 || position::path(&params.uri).is_none() {
        return true;
    }
    let available = LIMIT.saturating_sub(map.values().map(|p| p.diagnostics.len()).sum());
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
        map.insert(params.uri.as_str().into(), params);
    }
    limited
}

#[cfg(test)]
mod tests {
    use super::{HashMap, Rope, Synced, current, for_editor, retain_bounded};
    use lsp_types::*;
    #[test]
    fn stale_diagnostic_versions_and_unsynchronized_edits_are_not_displayed() {
        let text = Rope::from("const café = '😀';");
        let synced = Synced {
            text: text.clone(),
            version: 3,
        };
        assert!(current(&synced, &text, Some(3)));
        assert!(current(&synced, &text, None));
        assert!(!current(&synced, &text, Some(2)));
        assert!(!current(&synced, &text, Some(4)));
        assert!(!current(&synced, &Rope::from("changed"), Some(3)));
    }
    #[test]
    fn editor_messages_link_only_to_the_web_and_ranges_use_scalar_columns() {
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(0, 2), Position::new(0, 3)),
            message: "see [notes](file:///x.command) or <https://docs.rs>".into(),
            ..Default::default()
        };
        let [shown] = <[Diagnostic; 1]>::try_from(for_editor("😀x", &[diagnostic])).unwrap();
        assert_eq!(
            shown.message,
            r"see \[notes](file:///x.command) or <https://docs.rs>"
        );
        assert_eq!(
            shown.range,
            Range::new(Position::new(0, 1), Position::new(0, 2))
        );
    }
    #[test]
    fn diagnostics_are_bounded_and_an_empty_publication_clears_the_file() {
        let uri: Uri = "file:///work/a.ts".parse().unwrap();
        let mut map = HashMap::new();
        assert!(retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(uri.clone(), vec![Diagnostic::default(); 5_001], Some(1))
        ));
        assert_eq!(map[uri.as_str()].diagnostics.len(), 5_000);
        let other: Uri = "file:///work/b.ts".parse().unwrap();
        assert!(retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(other.clone(), vec![Diagnostic::default()], None)
        ));
        assert!(!map.contains_key(other.as_str()));
        assert!(!retain_bounded(
            &mut map,
            PublishDiagnosticsParams::new(uri, Vec::new(), Some(2))
        ));
        assert!(map.is_empty());
    }
}
