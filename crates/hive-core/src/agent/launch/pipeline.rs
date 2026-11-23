//! Role-specific arguments for interactive local pipeline sessions.
use super::{AgentKind, LaunchError, ResumeSession, valid_session_id};
use crate::pipeline::{PermissionMode, Profile, Ticket};
use std::path::PathBuf;

/// Access and handoff expected from this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineRole {
    /// Produce the plan before writing code.
    Plan,
    /// Implement an approved assignment.
    Develop,
    /// Inspect work across repositories and return findings.
    Review,
}

impl PipelineRole {
    /// Mode for a stage whose recipe chose none: only a developer may change files.
    pub fn default_mode(self) -> PermissionMode {
        match self {
            Self::Develop => PermissionMode::CliDefault,
            Self::Plan | Self::Review => PermissionMode::Plan,
        }
    }
}

/// A live agent bound to one pipeline attempt.
#[derive(Clone, Debug)]
pub struct PipelineLaunch {
    /// Local CLI, model and reasoning override.
    pub profile: Profile,
    /// Session access policy.
    pub role: PipelineRole,
    /// Other participating worktrees besides the launch directory.
    pub directories: Vec<PathBuf>,
    /// Task handoff. Empty only for a user-requested resume.
    pub prompt: String,
    /// Identifies the one attempt this process may complete.
    pub ticket: Ticket,
    /// Exact conversation for an explicit resume, or a preallocated OpenCode session.
    pub resume: Option<ResumeSession>,
    /// Folder of the images pasted into the task, when it refers to any. It lies outside
    /// the worktrees, so a CLI that reads only the folders it was given needs it named.
    pub image_directory: Option<PathBuf>,
}

/// The arguments that put `agent` in `mode`, as each CLI's `--help` names them (checked
/// 2026-11-23: claude 2.1.330, codex 0.171.0, agy 1.5.15, opencode 2.4.0). `None` when the
/// CLI has no such mode: `opencode mini` takes only `--agent`, so OpenCode offers just Plan.
fn mode_arguments(agent: AgentKind, mode: PermissionMode) -> Option<&'static [&'static str]> {
    use PermissionMode::*;
    match (agent, mode) {
        (_, CliDefault) => Some(&[]),
        (AgentKind::ClaudeCode, Plan) => Some(&["--permission-mode", "plan"]),
        (AgentKind::ClaudeCode, AcceptEdits) => Some(&["--permission-mode", "acceptEdits"]),
        (AgentKind::ClaudeCode, Auto) => Some(&["--permission-mode", "auto"]),
        (AgentKind::ClaudeCode, Bypass) => Some(&["--permission-mode", "bypassPermissions"]),
        (AgentKind::Codex, Plan) => Some(&["--sandbox", "read-only"]),
        (AgentKind::Codex, AcceptEdits) => Some(&[
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "on-request",
        ]),
        (AgentKind::Codex, Auto) => Some(&["--approve-for-me"]),
        (AgentKind::Codex, Bypass) => Some(&["--dangerously-bypass-approvals-and-sandbox"]),
        (AgentKind::Antigravity, Plan) => Some(&["--mode", "plan"]),
        (AgentKind::Antigravity, AcceptEdits) => Some(&["--mode", "accept-edits"]),
        (AgentKind::Antigravity, Auto) => Some(&["--mode", "auto"]),
        (AgentKind::Antigravity, Bypass) => Some(&["--dangerously-skip-permissions"]),
        (AgentKind::OpenCode, Plan) => Some(&["--agent", "plan"]),
        _ => None,
    }
}

/// Modes `agent` can be launched in, in [`PermissionMode::ALL`] order.
pub fn pipeline_modes(agent: AgentKind) -> Vec<PermissionMode> {
    PermissionMode::ALL
        .into_iter()
        .filter(|&mode| mode_arguments(agent, mode).is_some())
        .collect()
}

/// Choices offered by the adapter. An empty entry selects the provider default.
pub fn pipeline_efforts(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::Codex => &[
            "", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ],
        AgentKind::ClaudeCode | AgentKind::Antigravity | AgentKind::OpenCode => {
            &["", "low", "medium", "high", "xhigh", "max"]
        }
        _ => &[],
    }
}

/// Validate the session overrides before creating any worktrees or PTYs.
pub fn validate_pipeline_profile(profile: &Profile) -> Result<(), LaunchError> {
    profile
        .validate()
        .map_err(|error| LaunchError::Pipeline(error.to_string()))?;
    if !pipeline_efforts(profile.agent).contains(&profile.effort.as_str()) {
        return Err(LaunchError::Pipeline(format!(
            "Unsupported reasoning effort for {}",
            profile.agent.name()
        )));
    }
    if let Some(mode) = profile.mode
        && mode_arguments(profile.agent, mode).is_none()
    {
        return Err(LaunchError::Pipeline(format!(
            "Unsupported permission mode for {}",
            profile.agent.name()
        )));
    }
    if profile.agent == AgentKind::OpenCode {
        if !profile.model.is_empty()
            && !profile
                .model
                .split_once('/')
                .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
        {
            return Err(LaunchError::Pipeline(
                "Use a provider/model ID for OpenCode".into(),
            ));
        }
        if !profile.effort.is_empty() && (profile.model.is_empty() || profile.model.contains('#')) {
            return Err(LaunchError::Pipeline(
                "Choose an OpenCode model without a #variant before selecting effort".into(),
            ));
        }
    }
    Ok(())
}

impl PipelineLaunch {
    pub(super) fn arguments(&self) -> Result<Vec<String>, LaunchError> {
        validate_pipeline_profile(&self.profile)?;
        if self.role == PipelineRole::Develop && !self.directories.is_empty() {
            return Err(LaunchError::Pipeline(
                "Development needs a single worktree".into(),
            ));
        }
        if self.prompt.is_empty() && self.resume.is_none() {
            return Err(LaunchError::Pipeline("A new attempt needs a task".into()));
        }
        let agent = self.profile.agent;
        let mut args = Vec::new();
        if agent == AgentKind::OpenCode {
            args.push("mini".into());
        }
        if let Some(session) = &self.resume {
            if agent != session.agent() || !valid_session_id(session.id()) {
                return Err(LaunchError::InvalidSession);
            }
            let flag = match agent {
                AgentKind::Codex => "resume",
                AgentKind::ClaudeCode => "--resume",
                AgentKind::OpenCode => "--session",
                AgentKind::Antigravity => "--conversation",
                _ => return Err(LaunchError::Unsupported),
            };
            args.extend([flag.to_owned(), session.id().to_owned()]);
        }
        if !self.profile.model.is_empty() {
            let mut model = self.profile.model.clone();
            if agent == AgentKind::OpenCode && !self.profile.effort.is_empty() {
                model.push('#');
                model.push_str(&self.profile.effort);
            }
            args.extend(["--model".into(), model]);
        }
        if !self.profile.effort.is_empty() {
            match agent {
                AgentKind::Codex => args.extend([
                    "-c".into(),
                    format!("model_reasoning_effort={}", self.profile.effort),
                ]),
                AgentKind::ClaudeCode | AgentKind::Antigravity => {
                    args.extend(["--effort".into(), self.profile.effort.clone()])
                }
                _ => {}
            }
        }
        for directory in &self.directories {
            let path = directory
                .to_str()
                .filter(|_| directory.is_absolute())
                .ok_or_else(|| {
                    LaunchError::Pipeline(
                        "Agents need an absolute UTF-8 path for each worktree".into(),
                    )
                })?;
            // Codex --add-dir enables writes. Its read-only sandbox can already read these
            // paths; OpenCode's plan agent can request external-directory read permission.
            if matches!(agent, AgentKind::ClaudeCode | AgentKind::Antigravity) {
                args.extend(["--add-dir".into(), path.into()]);
            }
        }
        if let Some(directory) = &self.image_directory
            && matches!(agent, AgentKind::ClaudeCode | AgentKind::Antigravity)
        {
            let path = directory
                .to_str()
                .filter(|_| directory.is_absolute())
                .ok_or_else(|| {
                    LaunchError::Pipeline(
                        "Agents need an absolute UTF-8 path for the task images".into(),
                    )
                })?;
            args.extend(["--add-dir".into(), path.into()]);
        }
        let mode = self.profile.mode.unwrap_or(self.role.default_mode());
        let flags = mode_arguments(agent, mode).ok_or(LaunchError::Unsupported)?;
        args.extend(flags.iter().map(|flag| (*flag).to_owned()));
        if !self.prompt.is_empty() {
            let ticket = serde_json::to_string(&self.ticket)
                .map_err(|e| LaunchError::Pipeline(e.to_string()))?;
            let mut prompt = match self.role {
                PipelineRole::Plan => "Plan the task without changing project files. Ask the user if you need clarification.",
                PipelineRole::Review => "Inspect the task without changing project files.",
                PipelineRole::Develop => "Implement the assignment within your assigned worktree.",
            }.to_owned();
            prompt.push('\n');
            prompt.push_str(&self.prompt);
            for directory in &self.directories {
                prompt.push_str("\nAdditional participating worktree: ");
                prompt.push_str(&directory.display().to_string());
            }
            prompt.push_str(&format!("\nFinish this step by calling the Hive report tool with ticket {ticket} and your typed result, or run hive agent report '{ticket}' '<result JSON>'. No step advances until Hive acknowledges your report."));
            match agent {
                AgentKind::OpenCode => args.push("--prompt".into()),
                AgentKind::Antigravity => args.push("--prompt-interactive".into()),
                _ => {}
            }
            args.push(prompt);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::launch::{AGENTS, Launch};

    fn launch(agent: AgentKind, role: PipelineRole) -> PipelineLaunch {
        PipelineLaunch {
            role,
            profile: Profile {
                agent,
                model: if agent == AgentKind::OpenCode {
                    "vendor/model".into()
                } else {
                    "model".into()
                },
                effort: "high".into(),
                mode: None,
            },
            directories: vec![],
            prompt: "Inspect this change".into(),
            ticket: Ticket {
                run: 3,
                attempt: 2,
                lane: None,
            },
            resume: None,
            image_directory: None,
        }
    }

    #[test]
    fn task_images_are_readable_by_clis_that_read_only_named_folders() {
        for role in [PipelineRole::Plan, PipelineRole::Develop] {
            for agent in AGENTS {
                let mut pipeline = launch(agent, role);
                pipeline.image_directory = Some("/data/task-images".into());
                let args = Launch::Pipeline(pipeline)
                    .with_known_session()
                    .local_arguments(None)
                    .unwrap();
                let named = args
                    .windows(2)
                    .any(|pair| pair[0] == "--add-dir" && pair[1] == "/data/task-images");
                // Codex --add-dir would grant writes, and its sandbox reads any path already.
                assert_eq!(
                    named,
                    matches!(agent, AgentKind::ClaudeCode | AgentKind::Antigravity),
                    "{agent:?} {role:?}"
                );
                // The folder is not a worktree the agent takes part in.
                assert!(!args.iter().any(|a| a.contains("participating worktree")));
            }
        }
    }

    #[test]
    fn a_relative_image_folder_is_refused() {
        let mut pipeline = launch(AgentKind::ClaudeCode, PipelineRole::Plan);
        pipeline.image_directory = Some("task-images".into());
        assert!(
            Launch::Pipeline(pipeline)
                .with_known_session()
                .local_arguments(None)
                .is_err()
        );
    }

    #[test]
    fn all_four_interactive_planners_keep_permissions_and_receive_the_ticket() {
        for agent in AGENTS {
            let mut pipeline = launch(agent, PipelineRole::Plan);
            pipeline.directories.push("/repo/another worktree".into());
            let args = Launch::Pipeline(pipeline)
                .with_known_session()
                .local_arguments(None)
                .unwrap();
            assert!(args.iter().any(|a| a.contains("hive agent report")
                && a.contains("\"attempt\":2")
                && a.contains("/repo/another worktree")));
            assert!(!args.iter().any(|a| a.contains("bypass") || a == "--auto"));
            let matches =
                |name: &str, value: &str| args.windows(2).any(|pair| pair == [name, value]);
            match agent {
                AgentKind::Codex => {
                    assert!(
                        matches("--sandbox", "read-only")
                            && matches("-c", "model_reasoning_effort=high")
                    );
                    assert!(!args.iter().any(|a| a == "--add-dir"));
                }
                AgentKind::Antigravity => assert!(
                    matches("--mode", "plan")
                        && matches("--effort", "high")
                        && matches("--add-dir", "/repo/another worktree")
                ),
                AgentKind::ClaudeCode => assert!(
                    matches("--permission-mode", "plan")
                        && matches("--effort", "high")
                        && matches("--add-dir", "/repo/another worktree")
                ),
                AgentKind::OpenCode => assert!(
                    args[0] == "mini"
                        && matches("--agent", "plan")
                        && matches("--model", "vendor/model#high")
                ),
                _ => unreachable!(),
            }
        }
    }
    #[test]
    fn resuming_a_review_does_not_resubmit_its_prompt_or_drop_its_permissions() {
        for agent in AGENTS {
            let mut pipeline = launch(agent, PipelineRole::Review);
            pipeline.prompt.clear();
            pipeline.resume = ResumeSession::new(agent, "ses_preexisting");
            let args = pipeline.arguments().unwrap();
            assert!(args.iter().any(|arg| arg == "ses_preexisting"));
            assert!(args.iter().any(|arg| arg == "read-only" || arg == "plan"));
            assert!(
                !args
                    .iter()
                    .any(|arg| arg.contains("hive agent report") || arg == "--prompt")
            );
        }
    }
    #[test]
    fn a_continued_developer_resumes_its_conversation_and_receives_the_new_ticket() {
        for agent in AGENTS {
            let mut pipeline = launch(agent, PipelineRole::Develop);
            pipeline.prompt = "Resolve the finding".into();
            pipeline.resume = ResumeSession::new(agent, "ses_earlier_round");
            let args = pipeline.arguments().unwrap();
            let session = args.iter().position(|arg| arg == "ses_earlier_round");
            let message = args.iter().position(|arg| {
                arg.contains("Resolve the finding")
                    && arg.contains("hive agent report")
                    && arg.contains("\"attempt\":2")
            });
            assert!(
                session.is_some() && session < message,
                "{agent:?}: {args:?}"
            );
            assert!(
                !args.iter().any(|arg| arg == "read-only" || arg == "plan"),
                "{agent:?}: {args:?}"
            );
        }
    }
    /// The flags `mode` adds to `agent`'s launch in `role`, without the task prompt.
    fn mode_flags(agent: AgentKind, role: PipelineRole, mode: PermissionMode) -> Vec<String> {
        let mut pipeline = launch(agent, role);
        pipeline.profile.mode = Some(mode);
        let mut with_mode = pipeline.arguments().unwrap();
        pipeline.profile.mode = Some(PermissionMode::CliDefault);
        let without = pipeline.arguments().unwrap();
        // The prompt quotes the ticket; only the flags before it are of interest.
        with_mode.retain(|arg| !without.contains(arg));
        with_mode
    }

    #[test]
    fn each_mode_passes_exactly_the_flags_its_cli_names() {
        use PermissionMode::*;
        let expected: [(AgentKind, PermissionMode, &[&str]); 13] = [
            (AgentKind::ClaudeCode, Plan, &["--permission-mode", "plan"]),
            (
                AgentKind::ClaudeCode,
                AcceptEdits,
                &["--permission-mode", "acceptEdits"],
            ),
            (AgentKind::ClaudeCode, Auto, &["--permission-mode", "auto"]),
            (
                AgentKind::ClaudeCode,
                Bypass,
                &["--permission-mode", "bypassPermissions"],
            ),
            (AgentKind::Codex, Plan, &["--sandbox", "read-only"]),
            (
                AgentKind::Codex,
                AcceptEdits,
                &[
                    "--sandbox",
                    "workspace-write",
                    "--ask-for-approval",
                    "on-request",
                ],
            ),
            (AgentKind::Codex, Auto, &["--approve-for-me"]),
            (
                AgentKind::Codex,
                Bypass,
                &["--dangerously-bypass-approvals-and-sandbox"],
            ),
            (AgentKind::Antigravity, Plan, &["--mode", "plan"]),
            (
                AgentKind::Antigravity,
                AcceptEdits,
                &["--mode", "accept-edits"],
            ),
            (AgentKind::Antigravity, Auto, &["--mode", "auto"]),
            (
                AgentKind::Antigravity,
                Bypass,
                &["--dangerously-skip-permissions"],
            ),
            (AgentKind::OpenCode, Plan, &["--agent", "plan"]),
        ];
        for agent in AGENTS {
            let offered = pipeline_modes(agent);
            assert_eq!(offered[0], CliDefault, "{agent:?}");
            assert!(offered.contains(&Plan), "{agent:?}");
            for mode in PermissionMode::ALL.into_iter().skip(1) {
                let flags = expected
                    .iter()
                    .find(|(a, m, _)| *a == agent && *m == mode)
                    .map(|(_, _, flags)| *flags);
                assert_eq!(
                    offered.contains(&mode),
                    flags.is_some(),
                    "{agent:?} {mode:?}"
                );
                let Some(flags) = flags else {
                    // OpenCode's `mini` has no setting beyond its plan agent.
                    let mut pipeline = launch(agent, PipelineRole::Develop);
                    pipeline.profile.mode = Some(mode);
                    assert!(pipeline.arguments().is_err(), "{agent:?} {mode:?}");
                    continue;
                };
                for role in [
                    PipelineRole::Plan,
                    PipelineRole::Develop,
                    PipelineRole::Review,
                ] {
                    let added = mode_flags(agent, role, mode);
                    assert_eq!(added, flags, "{agent:?} {mode:?} {role:?}");
                    // Only the mode named for it turns every check off.
                    let wide = added.iter().any(|arg| {
                        arg.contains("bypass")
                            || arg.contains("dangerously")
                            || arg.contains("danger-full-access")
                            || arg == "--auto"
                    });
                    assert_eq!(wide, mode == Bypass, "{agent:?} {mode:?}");
                }
            }
        }
    }

    #[test]
    fn a_stage_without_a_saved_mode_keeps_its_roles_own() {
        for agent in AGENTS {
            for (role, mode) in [
                (PipelineRole::Plan, PermissionMode::Plan),
                (PipelineRole::Review, PermissionMode::Plan),
                (PipelineRole::Develop, PermissionMode::CliDefault),
            ] {
                assert_eq!(role.default_mode(), mode);
                let saved = launch(agent, role).arguments().unwrap();
                let mut chosen = launch(agent, role);
                chosen.profile.mode = Some(mode);
                assert_eq!(saved, chosen.arguments().unwrap(), "{agent:?} {role:?}");
            }
        }
    }

    #[test]
    fn a_chosen_mode_replaces_the_roles_own() {
        let planner = mode_flags(
            AgentKind::ClaudeCode,
            PipelineRole::Plan,
            PermissionMode::AcceptEdits,
        );
        assert_eq!(planner, ["--permission-mode", "acceptEdits"]);
        let mut planner = launch(AgentKind::ClaudeCode, PipelineRole::Plan);
        planner.profile.mode = Some(PermissionMode::CliDefault);
        assert!(!planner.arguments().unwrap().iter().any(|a| a == "plan"));
        let mut developer = launch(AgentKind::Codex, PipelineRole::Develop);
        developer.profile.mode = Some(PermissionMode::Plan);
        let args = developer.arguments().unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "read-only"])
        );
    }
    #[test]
    fn developer_stays_in_one_worktree_and_invalid_profiles_are_refused() {
        let mut pipeline = launch(AgentKind::Codex, PipelineRole::Develop);
        let args = pipeline.arguments().unwrap();
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--sandbox" || arg == "--add-dir")
        );
        pipeline.directories.push("/extra".into());
        assert!(pipeline.arguments().is_err());
        pipeline.directories.clear();
        pipeline.profile.effort = "--no-daemon".into();
        assert!(pipeline.arguments().is_err());
        for (model, effort) in [
            ("outside", ""),
            ("", "high"),
            ("provider/model#low", "high"),
            ("--auto", ""),
        ] {
            let profile = Profile {
                agent: AgentKind::OpenCode,
                model: model.into(),
                effort: effort.into(),
                mode: None,
            };
            assert!(validate_pipeline_profile(&profile).is_err());
        }
        let auto = Profile {
            agent: AgentKind::OpenCode,
            mode: Some(PermissionMode::Auto),
            ..Profile::default()
        };
        assert!(validate_pipeline_profile(&auto).is_err());
    }
}
