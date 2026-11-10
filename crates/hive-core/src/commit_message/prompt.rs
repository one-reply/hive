use super::{BASE_PROMPT, Error, PROMPT_LIMIT, validate_prompt};
use crate::git::commit_context::Context;
use std::borrow::Cow;

const PATCH: &str = "{stagedPatch}";

pub(super) fn render(template: &str, context: &Context) -> Result<String, Error> {
    fill(template, context.branch(), context.files(), |budget| {
        context.patch_within(budget)
    })
}

/// Expands `template`, asking for a patch no larger than what the rest of the prompt leaves.
fn fill<'a>(
    template: &str,
    branch: &str,
    files: &str,
    patch_within: impl FnOnce(usize) -> Cow<'a, str>,
) -> Result<String, Error> {
    validate_prompt(template)?;
    let values = |patch| {
        [
            ("{basePrompt}", BASE_PROMPT),
            ("{branch}", branch),
            ("{stagedFiles}", files),
            (PATCH, patch),
        ]
    };
    let without_patch = expand(template, &values(""))?;
    // Substituted values are never rescanned, so the template alone decides how often it is used.
    let uses = template.matches(PATCH).count();
    if uses == 0 {
        return Ok(without_patch);
    }
    let patch = patch_within(PROMPT_LIMIT.saturating_sub(without_patch.len()) / uses);
    expand(template, &values(&patch))
}

fn expand(template: &str, values: &[(&str, &str)]) -> Result<String, Error> {
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some((name, value)) = values.iter().find(|(name, _)| rest.starts_with(name)) {
            output.push_str(value);
            rest = &rest[name.len()..];
        } else {
            output.push('{');
            rest = &rest[1..];
        }
        validate_prompt_size(&output)?;
    }
    output.push_str(rest);
    validate_prompt(&output)?;
    Ok(output)
}

fn validate_prompt_size(output: &str) -> Result<(), Error> {
    if output.len() > PROMPT_LIMIT {
        validate_prompt(output)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn injected_values_are_not_expanded_again() {
        assert_eq!(
            expand(
                "{branch} {stagedPatch} {unknown}",
                &[
                    ("{branch}", "feature/{stagedPatch}"),
                    ("{stagedPatch}", "code {branch}")
                ]
            )
            .unwrap(),
            "feature/{stagedPatch} code {branch} {unknown}"
        );
    }
    #[test]
    fn expanded_budget_is_enforced() {
        assert!(
            expand(
                "{patch}{patch}",
                &[("{patch}", &"a".repeat(PROMPT_LIMIT / 2 + 1))]
            )
            .is_err()
        );
    }
    #[test]
    fn patch_budget_is_what_the_rest_of_the_prompt_leaves() {
        let mut offered = None;
        let prompt = fill(
            "{branch}: {stagedFiles}\n{stagedPatch}",
            "main",
            "a\nb",
            |budget| {
                offered = Some(budget);
                "x".repeat(budget).into()
            },
        )
        .unwrap();
        assert_eq!(offered, Some(PROMPT_LIMIT - "main: a\nb\n".len()));
        assert_eq!(prompt.len(), PROMPT_LIMIT);
    }
    #[test]
    fn repeated_patch_variable_shares_the_budget() {
        let mut offered = None;
        let prompt = fill("{stagedPatch}|{stagedPatch}", "main", "", |budget| {
            offered = Some(budget);
            "x".repeat(budget).into()
        })
        .unwrap();
        assert_eq!(offered, Some((PROMPT_LIMIT - 1) / 2));
        assert!(prompt.len() <= PROMPT_LIMIT);
    }
    #[test]
    fn template_without_the_patch_variable_never_asks_for_it() {
        let prompt = fill("{branch} {stagedFiles}", "main", "a", |_| {
            unreachable!("the template has no patch variable")
        })
        .unwrap();
        assert_eq!(prompt, "main a");
    }
    #[test]
    fn patch_larger_than_its_budget_is_rejected_instead_of_truncated() {
        let result = fill("{stagedPatch}", "main", "", |budget| {
            "x".repeat(budget + 1).into()
        });
        assert!(matches!(result, Err(Error::Invalid(_))));
    }
}
