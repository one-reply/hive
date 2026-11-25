//! Text-only, client-requested workspace edits. Resource operations cannot enter an editor buffer.
use lsp_types::{
    DocumentChangeOperation, DocumentChanges, OneOf, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

/// The edits a server computed for one document and its optional synchronized version.
pub struct DocumentEdits {
    /// Original server URI.
    pub uri: Uri,
    /// Expected LSP version for an open document.
    pub version: Option<i32>,
    /// Simultaneous edits to this version.
    pub edits: Vec<TextEdit>,
}

fn text_edits(edit: TextDocumentEdit) -> Result<DocumentEdits, &'static str> {
    let edits = edit
        .edits
        .into_iter()
        .map(|edit| match edit {
            OneOf::Left(edit) => Ok(edit),
            OneOf::Right(_) => Err("Annotated edits are unsupported"),
        })
        .collect::<Result<_, _>>()?;
    Ok(DocumentEdits {
        uri: edit.text_document.uri,
        version: edit.text_document.version,
        edits,
    })
}

/// Extracts plain text edits or rejects the entire operation before making any change.
pub fn documents(edit: WorkspaceEdit) -> Result<Vec<DocumentEdits>, &'static str> {
    // lsp-types can deserialize an annotated edit as a plain TextEdit with extra fields.
    if edit
        .change_annotations
        .is_some_and(|annotations| !annotations.is_empty())
    {
        return Err("Annotated edits are unsupported");
    }
    match (edit.document_changes, edit.changes) {
        (None, changes) => Ok(changes
            .unwrap_or_default()
            .into_iter()
            .map(|(uri, edits)| DocumentEdits {
                uri,
                version: None,
                edits,
            })
            .collect()),
        (Some(_), Some(_)) => Err("The server returned two conflicting edit lists"),
        (Some(DocumentChanges::Edits(items)), _) => items.into_iter().map(text_edits).collect(),
        (Some(DocumentChanges::Operations(items)), _) => items
            .into_iter()
            .map(|item| match item {
                DocumentChangeOperation::Edit(edit) => text_edits(edit),
                DocumentChangeOperation::Op(_) => {
                    Err("This change also creates, moves or deletes files")
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_a_mixed_rename_before_applying_any_text() {
        let edit = serde_json::from_value(serde_json::json!({"documentChanges":[
            {"textDocument":{"uri":"file:///a","version":3},"edits":[]},
            {"kind":"rename","oldUri":"file:///a","newUri":"file:///b"}
        ]}))
        .unwrap();
        assert!(documents(edit).is_err());
    }
    #[test]
    fn refuses_an_edit_requiring_separate_confirmation() {
        let edit = serde_json::from_value(serde_json::json!({
            "changeAnnotations": {"a": {"label": "Changes public API", "needsConfirmation": true}},
            "documentChanges": [{
                "textDocument": {"uri": "file:///a", "version": null},
                "edits": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                    "newText": "b", "annotationId": "a"
                }]
            }]
        })).unwrap();
        assert!(documents(edit).is_err());
    }

    #[test]
    fn retains_document_version() {
        let edit = serde_json::from_value(serde_json::json!({"documentChanges":[
            {"textDocument":{"uri":"file:///a","version":3},"edits":[]}
        ]}))
        .unwrap();
        assert_eq!(documents(edit).unwrap()[0].version, Some(3));
    }
}
