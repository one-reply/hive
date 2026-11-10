//! Resolving an unmerged file by taking one whole side: wording and the confirmed action.
use crate::state::git::Git;
use crate::ui::git_controls::{self, Confirmation};
use gpui_kit::{App, Entity, Window};
use hive_core::git::changes::Action;
use hive_core::git::conflicts::{Conflict, Side};
use hive_core::projects::Target;

/// Whether taking `side` keeps a file; an unknown conflict is assumed to have both versions.
fn keeps(kind: Option<Conflict>, side: Side) -> bool {
    kind.is_none_or(|kind| kind.has(side))
}

/// Git's "ours" is what the widget calls the current change, "theirs" the incoming one.
fn name(side: Side) -> &'static str {
    match side {
        Side::Ours => "current",
        Side::Theirs => "incoming",
    }
}

/// The whole-file choices worth offering: both delete the file when neither side has it.
pub(crate) fn choices(kind: Option<Conflict>) -> &'static [Side] {
    match kind {
        Some(Conflict::BothDeleted) => &[Side::Ours],
        _ => &[Side::Ours, Side::Theirs],
    }
}

pub(crate) fn label(kind: Option<Conflict>, side: Side) -> &'static str {
    match (keeps(kind, side), side) {
        (false, _) => "Delete File",
        (true, Side::Ours) => "Use Current File",
        (true, Side::Theirs) => "Use Incoming File",
    }
}

pub(crate) fn tooltip(kind: Option<Conflict>, side: Side) -> String {
    if keeps(kind, side) {
        format!("Use the {} version of the whole file", name(side))
    } else {
        format!("Delete the file, as on the {} side", name(side))
    }
}

fn confirmation(path: &str, kind: Option<Conflict>, side: Side) -> Confirmation {
    let side_name = name(side);
    if keeps(kind, side) {
        Confirmation {
            title: "Use Whole File",
            description: format!(
                "{path} will be replaced with its {side_name} version and staged. Changes from the other side and edits made to the working file are lost and cannot be recovered."
            ),
            ok_text: label(kind, side),
            danger: true,
        }
    } else {
        Confirmation {
            title: "Delete File",
            description: format!(
                "{path} was deleted on the {side_name} side. It will be deleted here too, and the other side's version is lost."
            ),
            ok_text: "Delete File",
            danger: true,
        }
    }
}

/// Asks before replacing the working file with one side, then resolves the conflict with it.
pub(crate) fn take(
    git: Entity<Git>,
    target: Target,
    path: &str,
    kind: Option<Conflict>,
    side: Side,
    window: &mut Window,
    cx: &mut App,
) {
    git_controls::confirm_at(
        git,
        target,
        Action::ResolveWith {
            path: path.to_owned(),
            side,
        },
        confirmation(path, kind, side),
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::{choices, confirmation, label, tooltip};
    use hive_core::git::conflicts::{Conflict, Side};

    #[test]
    fn a_side_without_the_file_is_offered_as_deletion() {
        assert_eq!(label(None, Side::Ours), "Use Current File");
        assert_eq!(
            label(Some(Conflict::BothModified), Side::Theirs),
            "Use Incoming File"
        );
        // They deleted the file: the incoming side has nothing to use.
        let kind = Some(Conflict::DeletedByThem);
        assert_eq!(label(kind, Side::Ours), "Use Current File");
        assert_eq!(label(kind, Side::Theirs), "Delete File");
        assert_eq!(
            tooltip(kind, Side::Theirs),
            "Delete the file, as on the incoming side"
        );
        assert_eq!(
            tooltip(kind, Side::Ours),
            "Use the current version of the whole file"
        );
        for side in [Side::Ours, Side::Theirs] {
            assert_eq!(label(Some(Conflict::BothDeleted), side), "Delete File");
        }
        // Two buttons that both delete the file would be one choice shown twice.
        assert_eq!(choices(Some(Conflict::BothDeleted)), [Side::Ours]);
        assert_eq!(choices(Some(Conflict::DeletedByUs)).len(), 2);
        assert_eq!(choices(None).len(), 2);
    }

    #[test]
    fn confirmation_names_the_file_and_what_is_lost() {
        let keep = confirmation("src/a.rs", Some(Conflict::BothModified), Side::Theirs);
        assert!(keep.danger && keep.ok_text == "Use Incoming File");
        assert!(keep.description.contains("src/a.rs") && keep.description.contains("incoming"));
        let delete = confirmation("src/a.rs", Some(Conflict::DeletedByUs), Side::Ours);
        assert!(delete.danger && delete.ok_text == "Delete File");
        assert!(delete.description.contains("deleted on the current side"));
    }
}
