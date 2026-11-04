use super::{BASE_PROMPT, Error, validate_prompt};
use crate::git::commit_context::Context;

pub(super) fn render(template: &str, context: &Context) -> Result<String, Error> {
    validate_prompt(template)?;
    let values = [
        ("{basePrompt}", BASE_PROMPT),
        ("{branch}", context.branch()),
        ("{stagedFiles}", context.files()),
        ("{stagedPatch}", context.patch()),
    ];
    expand(template, &values)
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
    if output.len() > super::PROMPT_LIMIT {
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
                &[("{patch}", &"a".repeat(super::super::PROMPT_LIMIT / 2 + 1))]
            )
            .is_err()
        );
    }
}
