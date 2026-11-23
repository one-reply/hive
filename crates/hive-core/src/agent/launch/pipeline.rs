//! Role-specific arguments for interactive local pipeline sessions.
use super::{AgentKind, LaunchError, ResumeSession, valid_session_id};
use crate::pipeline::{EditApproval, Profile, Ticket};
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
    /// Whether a developer edits files in its worktree without asking; other roles are
    /// read-only whatever this says.
    pub edits: EditApproval,
    /// Folder of the images pasted into the task, when it refers to any. It lies outside
    /// the worktrees, so a CLI that reads only the folders it was given needs it named.
    pub image_directory: Option<PathBuf>,
}

/// The arguments that let `agent` edit files in its worktree without a prompt while it still
/// asks for anything beyond that. `None` when the CLI has no such setting: OpenCode offers
/// only `--auto`, which approves every permission, and Hive never passes a blanket approval.
fn accept_edits_arguments(agent: AgentKind) -> Option<&'static [&'static str]> {
    match agent {
        AgentKind::ClaudeCode => Some(&["--permission-mode", "acceptEdits"]),
        AgentKind::Codex => Some(&[
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "on-request",
        ]),
        AgentKind::Antigravity => Some(&["--mode", "accept-edits"]),
        _ => None,
    }
}

/// Whether [`EditApproval::AcceptEdits`] changes how `agent` is launched.
pub fn supports_accept_edits(agent: AgentKind) -> bool {
    accept_edits_arguments(agent).is_some()
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
        let read_only = self.role != PipelineRole::Develop;
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
        if read_only {
            let (flag, value) = match agent {
                AgentKind::ClaudeCode => ("--permission-mode", "plan"),
                AgentKind::Codex => ("--sandbox", "read-only"),
                AgentKind::OpenCode => ("--agent", "plan"),
                AgentKind::Antigravity => ("--mode", "plan"),
                _ => return Err(LaunchError::Unsupported),
            };
            args.extend([flag.into(), value.into()]);
        } else if self.edits == EditApproval::AcceptEdits
            && let Some(flags) = accept_edits_arguments(agent)
        {
            args.extend(flags.iter().map(|flag| (*flag).to_owned()));
        }
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
            },
            directories: vec![],
            prompt: "Inspect this change".into(),
            ticket: Ticket {
                run: 3,
                attempt: 2,
                lane: None,
            },
            resume: None,
            edits: EditApproval::CliDefault,
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
    #[test]
    fn accepting_edits_is_opt_in_for_developers_only_and_never_a_blanket_approval() {
        for agent in AGENTS {
            let flags = |role, edits| {
                let mut pipeline = launch(agent, role);
                pipeline.edits = edits;
                let prompt = pipeline.prompt.clone();
                let mut args = pipeline.arguments().unwrap();
                // The prompt quotes the ticket; only the flags before it are of interest.
                args.retain(|arg| !arg.contains(&prompt));
                args
            };
            let ask = flags(PipelineRole::Develop, EditApproval::CliDefault);
            let accept = flags(PipelineRole::Develop, EditApproval::AcceptEdits);
            let added: Vec<_> = accept.iter().filter(|arg| !ask.contains(arg)).collect();
            match agent {
                AgentKind::ClaudeCode => assert_eq!(added, ["--permission-mode", "acceptEdits"]),
                AgentKind::Codex => assert_eq!(
                    added,
                    [
                        "--sandbox",
                        "workspace-write",
                        "--ask-for-approval",
                        "on-request"
                    ]
                ),
                AgentKind::Antigravity => assert_eq!(added, ["--mode", "accept-edits"]),
                AgentKind::OpenCode => assert!(added.is_empty()),
                _ => unreachable!(),
            }
            assert_eq!(supports_accept_edits(agent), !added.is_empty());
            for arg in &accept {
                assert!(
                    !arg.contains("bypass")
                        && !arg.contains("dangerously")
                        && !arg.contains("danger-full-access")
                        && arg != "--auto"
                        && arg != "never",
                    "{agent:?}: {arg}"
                );
            }
            // A reviewer stays read-only even if a lane's setting leaks into its launch.
            let review = flags(PipelineRole::Review, EditApproval::AcceptEdits);
            assert_eq!(
                review,
                flags(PipelineRole::Review, EditApproval::CliDefault)
            );
        }
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
            };
            assert!(validate_pipeline_profile(&profile).is_err());
        }
    }
}
