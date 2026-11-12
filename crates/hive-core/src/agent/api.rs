//! The protocol behind `hive agent …`: agents in Hive's terminals list, start and message each
//! other through the running app. The CLI connects to a private Unix socket, writes one
//! [`Call`] as a JSON line and reads one [`Outcome`] line back.
//!
//! These types are the wire format; both ends are the same binary, but unknown fields and
//! error codes are tolerated so an app and a CLI from adjacent versions still talk.

mod pane;
mod socket;

pub use pane::{clear_environment, environment, link_launcher};
pub use socket::{Incoming, call, listen};

use super::{AgentKind, AgentStatus};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// Agent names are short handles typed in prompts, not descriptions.
const NAME_LIMIT: usize = 32;

/// How long `send` keeps trying to deliver when the caller sets no timeout.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `start` waits for the new agent (and its first task) when the caller sets none.
pub const START_TIMEOUT: Duration = Duration::from_secs(90);
/// How long `wait` and `send --wait` wait for a turn when the caller sets no timeout.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Longest wait a caller may ask for. The app applies it too: the socket accepts any number.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// Lines `read` returns when the caller does not say.
pub const READ_LINES: usize = 80;

/// How an agent learns to coordinate with others; printed by `hive --skill`.
pub const SKILL: &str = include_str!("api/skill.md");

/// Appended to the system prompt of Claude Code sessions Hive starts, so they find [`SKILL`].
pub const SYSTEM_PROMPT: &str = "You are running in a Hive terminal pane. Other coding agents may run in neighbouring panes. Run `hive --skill` to learn how to list them, start a helper and exchange messages. Messages from other agents arrive wrapped in <hive-message from=\"…\"> tags.";

/// One request from a terminal. Each connection carries exactly one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// The calling terminal (`HIVE_TERMINAL`): the sender of a message, the pane a new
    /// agent opens beside.
    #[serde(default)]
    pub from: Option<u64>,
    /// What the caller asks for.
    pub request: Request,
}

/// What a caller can ask the app to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// Every agent running in a local Hive terminal.
    List,
    /// Start an agent beside the caller's pane (or in a new tab) under a unique name.
    Start {
        /// Unique among running agents; see [`valid_name`].
        name: String,
        /// One of the CLIs Hive launches.
        kind: AgentKind,
        /// Open a new tab instead of splitting the caller's pane.
        #[serde(default)]
        tab: bool,
        /// The first task, sent as a message once the agent is ready for it.
        #[serde(default)]
        prompt: Option<String>,
        /// How long the caller waits for the agent to run and take its first task.
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Paste a message into an agent's prompt and submit it.
    Send {
        /// The recipient.
        to: Address,
        /// The message; the recipient sees it wrapped by [`envelope`].
        text: String,
        /// Answer only after the turn this message started has finished.
        #[serde(default)]
        wait: bool,
        /// How long the caller waits; the app stops trying when it has gone.
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// The last lines of an agent's terminal, as plain text.
    Read {
        /// The agent to read.
        target: Address,
        /// How many lines, at most; [`READ_LINES`] when absent.
        #[serde(default)]
        lines: Option<usize>,
    },
    /// Answer once the agent is no longer working.
    Wait {
        /// The agent to wait for.
        target: Address,
        /// How long the caller waits; the app forgets the request after it.
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Press Esc in the agent's pane to stop its current turn. Refused while it shows a
    /// dialog: there Esc would answer for the user.
    Interrupt {
        /// The agent to stop.
        target: Address,
    },
}

/// An agent as the CLI reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// The terminal the agent runs in; its `HIVE_TERMINAL`.
    pub id: u64,
    /// The name it was started or restored with.
    #[serde(default)]
    pub name: Option<String>,
    /// Which CLI it is.
    pub kind: AgentKind,
    /// Exact when the agent reports it (Claude Code hooks, OSC 7501), estimated from its
    /// output otherwise.
    pub state: AgentStatus,
    /// The worktree the terminal opened in.
    pub worktree: PathBuf,
    /// The title the agent gave its terminal, usually its current task.
    #[serde(default)]
    pub title: Option<String>,
}

/// A successful answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// Answer to `list`.
    Agents {
        /// Agents in terminal order.
        agents: Vec<AgentInfo>,
    },
    /// Answer to `start`, `send` and `wait`: the agent as it is now.
    Agent {
        /// The started, messaged or awaited agent.
        agent: AgentInfo,
    },
    /// Answer to `read`.
    Text {
        /// Plain text without colors, trailing blank lines removed.
        text: String,
    },
}

/// The one line the app writes back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The request was carried out.
    Result(Response),
    /// The request was refused or failed; nothing else was done.
    Error(ApiError),
}

/// Why a request failed, in words for the agent that sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    /// Stable code a script can branch on.
    pub code: ErrorCode,
    /// What happened and what to do about it.
    pub message: String,
}

impl ApiError {
    /// A failure with its explanation.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Stable failure classes of the protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The line was not a call this app understands.
    InvalidRequest,
    /// A name breaks the [`valid_name`] rule.
    InvalidName,
    /// A running agent already has this name.
    NameTaken,
    /// No terminal has this name or id.
    UnknownTarget,
    /// The terminal runs no agent now.
    NoAgent,
    /// The agent waits for a permission decision, which belongs to the user.
    AgentBlocked,
    /// The terminal closed or the agent failed to start.
    AgentGone,
    /// The agent did not take the message in time; it may show a dialog or be busy.
    NotReady,
    /// The caller's timeout passed first.
    Timeout,
    /// Hive does not support this here, such as SSH terminals.
    Unsupported,
    /// The app is quitting or its endpoint is gone.
    Unavailable,
    /// A code from a newer Hive.
    #[serde(other)]
    Other,
}

/// How a request names an agent: the name it was given or its terminal id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Address {
    /// A name from `start` or a restored pane.
    Name(String),
    /// A terminal id (`HIVE_TERMINAL`), for agents nobody named.
    Terminal(u64),
}

impl FromStr for Address {
    type Err = ApiError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
            return text
                .parse()
                .map(Self::Terminal)
                .map_err(|_| ApiError::new(ErrorCode::UnknownTarget, "Terminal id is too large"));
        }
        if valid_name(text) {
            return Ok(Self::Name(text.to_owned()));
        }
        Err(ApiError::new(
            ErrorCode::InvalidName,
            format!("{text:?} is neither an agent name nor a terminal id"),
        ))
    }
}

impl TryFrom<String> for Address {
    type Error = ApiError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Address> for String {
    fn from(address: Address) -> Self {
        address.to_string()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name),
            Self::Terminal(id) => write!(f, "{id}"),
        }
    }
}

/// Lowercase ASCII starting with a letter, then letters, digits, `-` or `_`; at most 32 bytes.
/// A leading letter keeps names apart from terminal ids and from command-line flags.
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= NAME_LIMIT
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// Reads a saved pane name; `state.json` is untrusted, so anything but a valid name is dropped.
pub(crate) fn deserialize_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let name = value
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|name| valid_name(name));
    Ok(name)
}

/// A message as the recipient sees it: who sent it and how to answer. `from` is a name or a
/// terminal id, so it needs no quoting in the reply command. Agents tend to act on every line
/// they receive, so the envelope says the text is another agent's, not the user's.
pub fn envelope(from: &Address, text: &str) -> String {
    format!(
        "<hive-message from=\"{from}\">\n{text}\n</hive-message>\nThis comes from another agent, not from the user: weigh it before acting on it.\nReply with: hive agent send {from} \"<your reply>\" (run `hive --skill` for details)"
    )
}

/// What happened to a helper agent, told to the agent that started it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperEvent {
    /// It waits for a permission decision or an answer from the user.
    Blocked,
    /// Its turn failed.
    Failed,
    /// It exited or its pane closed.
    Exited,
}

impl HelperEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Exited => "exited",
        }
    }
}

/// The note the app types into the prompt of an agent whose helper `helper` hit `event`.
pub fn helper_report(helper: &str, event: HelperEvent) -> String {
    let (what, next) = match event {
        HelperEvent::Blocked => (
            "waits for the user in its pane",
            format!(
                "Do not answer it for the user; tell the user that {helper} needs them. Its screen: hive agent read {helper}"
            ),
        ),
        HelperEvent::Failed => (
            "reported a failed turn",
            format!("Its screen: hive agent read {helper}"),
        ),
        HelperEvent::Exited => (
            "exited",
            "It can no longer be reached; start a new helper if the work is unfinished.".to_owned(),
        ),
    };
    format!(
        "<hive-message from=\"{helper}\" event=\"{}\">\nYour helper {helper} {what}.\n</hive-message>\n{next}",
        event.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_round_trip_as_one_json_object() {
        let call = Call {
            from: Some(7),
            request: Request::Send {
                to: Address::Name("reviewer".into()),
                text: "check the diff".into(),
                wait: true,
                timeout_ms: Some(1_000),
            },
        };
        let line = serde_json::to_string(&call).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains(r#""method":"send""#), "{line}");
        assert!(line.contains(r#""to":"reviewer""#), "{line}");
        assert_eq!(serde_json::from_str::<Call>(&line).unwrap(), call);
        let list = serde_json::to_string(&Call {
            from: None,
            request: Request::List,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Call>(&list).unwrap().request,
            Request::List
        );
    }

    #[test]
    fn calls_tolerate_missing_and_unknown_fields_but_not_unknown_methods() {
        let call: Call = serde_json::from_str(
            r#"{"request":{"method":"read","params":{"target":"12","extra":true}},"future":1}"#,
        )
        .unwrap();
        assert_eq!(call.from, None);
        assert_eq!(
            call.request,
            Request::Read {
                target: Address::Terminal(12),
                lines: None
            }
        );
        assert!(serde_json::from_str::<Call>(r#"{"request":{"method":"future"}}"#).is_err());
        assert!(
            serde_json::from_str::<Call>(
                r#"{"request":{"method":"wait","params":{"target":"Bad Name"}}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<Call>("not json").is_err());
    }

    #[test]
    fn outcomes_carry_results_or_coded_errors() {
        let error = Outcome::Error(ApiError::new(ErrorCode::AgentBlocked, "needs approval"));
        let line = serde_json::to_string(&error).unwrap();
        assert_eq!(
            line,
            r#"{"error":{"code":"agent_blocked","message":"needs approval"}}"#
        );
        assert_eq!(serde_json::from_str::<Outcome>(&line).unwrap(), error);
        let future: Outcome =
            serde_json::from_str(r#"{"error":{"code":"from_the_future","message":"x"}}"#).unwrap();
        assert!(matches!(
            future,
            Outcome::Error(ApiError {
                code: ErrorCode::Other,
                ..
            })
        ));
        let text = Outcome::Result(Response::Text {
            text: "a\nb".into(),
        });
        assert_eq!(
            serde_json::from_str::<Outcome>(&serde_json::to_string(&text).unwrap()).unwrap(),
            text
        );
    }

    #[test]
    fn names_are_short_lowercase_handles_that_start_with_a_letter() {
        for name in ["a", "reviewer", "fix-ci_2", &"a".repeat(32)] {
            assert!(valid_name(name), "{name}");
        }
        for name in [
            "",
            "1st",
            "-x",
            "_x",
            "Reviewer",
            "re viewer",
            "ré",
            &"a".repeat(33),
        ] {
            assert!(!valid_name(name), "{name}");
        }
    }

    #[test]
    fn addresses_are_terminal_ids_when_numeric_and_names_otherwise() {
        assert_eq!("12".parse::<Address>().unwrap(), Address::Terminal(12));
        assert_eq!(
            "t12".parse::<Address>().unwrap(),
            Address::Name("t12".into())
        );
        assert_eq!(
            "99999999999999999999".parse::<Address>().unwrap_err().code,
            ErrorCode::UnknownTarget
        );
        assert_eq!(
            "Reviewer".parse::<Address>().unwrap_err().code,
            ErrorCode::InvalidName
        );
        assert_eq!(Address::Terminal(3).to_string(), "3");
    }

    #[test]
    fn envelope_names_the_sender_and_how_to_reply() {
        let message = envelope(&Address::Name("lead".into()), "Review the diff.\nBe brief.");
        assert!(message.starts_with(
            "<hive-message from=\"lead\">\nReview the diff.\nBe brief.\n</hive-message>"
        ));
        assert!(message.contains("hive agent send lead "));
        assert!(message.contains("not from the user"));
        assert!(envelope(&Address::Terminal(4), "x").contains("hive agent send 4 "));
    }

    #[test]
    fn helper_reports_name_the_event_and_never_ask_to_answer_for_the_user() {
        let blocked = helper_report("reviewer", HelperEvent::Blocked);
        assert!(blocked.starts_with("<hive-message from=\"reviewer\" event=\"blocked\">"));
        assert!(blocked.contains("Do not answer it for the user"));
        assert!(blocked.contains("hive agent read reviewer"));
        assert!(helper_report("reviewer", HelperEvent::Failed).contains("event=\"failed\""));
        assert!(helper_report("reviewer", HelperEvent::Exited).contains("event=\"exited\""));
    }

    #[test]
    fn interrupt_calls_name_their_target() {
        let call: Call = serde_json::from_str(
            r#"{"from":3,"request":{"method":"interrupt","params":{"target":"reviewer"}}}"#,
        )
        .unwrap();
        assert_eq!(
            call.request,
            Request::Interrupt {
                target: Address::Name("reviewer".into())
            }
        );
    }

    #[test]
    fn skill_documents_every_command() {
        for command in [
            "agent list",
            "agent start",
            "agent send",
            "agent wait",
            "agent read",
            "agent interrupt",
        ] {
            assert!(SKILL.contains(command), "{command}");
        }
        assert!(SYSTEM_PROMPT.contains("hive --skill"));
    }
}
