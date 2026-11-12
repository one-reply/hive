//! Command-line modes of the Hive binary that run without a window: Claude hook forwarding,
//! `hive --skill` and `hive agent …`, which agents in Hive's terminals use to reach the app.
use hive_core::agent::AgentKind;
use hive_core::agent::api::{self, Address, ApiError, Call, ErrorCode, Outcome, Request, Response};
use hive_core::agent::launch::AGENTS;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

/// Time for the app to answer after the request's own deadline passed.
const ANSWER_GRACE: Duration = Duration::from_secs(5);
/// `list`, `read` and `interrupt` answer from memory.
const QUICK_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
Usage:
  hive agent list
  hive agent start <name> --kind <claude|codex|opencode|agy> [--prompt <task>] [--tab]
  hive agent send <target> <text> [--wait]
  hive agent wait <target>
  hive agent read <target> [--lines <n>]
  hive agent interrupt <target>
Every command accepts --timeout <seconds>. `hive --skill` explains how agents use them.";

/// Runs a window-less mode named by `args`, or returns `None` to start the app.
pub(crate) fn run(args: &[OsString]) -> Option<ExitCode> {
    match args.get(1)?.to_str()? {
        "--agent-hook" => Some(forward_hook(&args[2..])),
        "--skill" => Some(print(api::SKILL.trim_end())),
        "agent" => Some(agent(&args[2..])),
        _ => None,
    }
}

fn forward_hook(args: &[OsString]) -> ExitCode {
    let terminal = args
        .get(1)
        .and_then(|terminal| terminal.to_str())
        .and_then(|terminal| terminal.parse().ok());
    if let (Some(endpoint), Some(terminal)) = (args.first(), terminal)
        && let Err(error) =
            hive_core::agent::hooks::forward(Path::new(endpoint), terminal, std::io::stdin().lock())
    {
        eprintln!("Hive hook: {error}");
    }
    // A failing hook must never block the agent it reports on.
    ExitCode::SUCCESS
}

fn agent(args: &[OsString]) -> ExitCode {
    let args: Option<Vec<String>> = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect();
    let invocation = match args.as_deref().map(parse) {
        Some(Ok(Parsed::Help)) => return print(USAGE),
        Some(Ok(Parsed::Call(invocation))) => invocation,
        Some(Err(problem)) => return usage_error(&problem),
        None => return usage_error("arguments must be UTF-8"),
    };
    match call(invocation) {
        Ok(Response::Text { text }) => print(&text),
        Ok(response) => match serde_json::to_string(&response) {
            Ok(line) => print(&line),
            Err(error) => failure(&ApiError::new(ErrorCode::Other, error.to_string())),
        },
        Err(error) => failure(&error),
    }
}

/// Reaches the app through the socket this terminal was given.
fn call(invocation: Invocation) -> Result<Response, ApiError> {
    let socket = std::env::var_os("HIVE_SOCKET")
        .filter(|socket| !socket.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unavailable,
                "Not running inside a Hive terminal: HIVE_SOCKET is not set",
            )
        })?;
    let from = std::env::var("HIVE_TERMINAL")
        .ok()
        .and_then(|terminal| terminal.parse().ok());
    let call = Call {
        from,
        request: invocation.request,
    };
    let outcome = api::call(Path::new(&socket), &call, invocation.timeout + ANSWER_GRACE).map_err(
        |error| match error.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => ApiError::new(
                ErrorCode::Timeout,
                "Hive did not answer in time; the request may still have taken effect",
            ),
            _ => ApiError::new(
                ErrorCode::Unavailable,
                format!("Could not reach Hive: {error}"),
            ),
        },
    )?;
    match outcome {
        Outcome::Result(response) => Ok(response),
        Outcome::Error(error) => Err(error),
    }
}

/// Writes the answer and a newline. Agents pipe it into `head`, which stops reading early:
/// that is not a failure, and `println!` would panic on it.
fn print(text: &str) -> ExitCode {
    match writeln!(std::io::stdout().lock(), "{text}") {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}

fn failure(error: &ApiError) -> ExitCode {
    let line = serde_json::to_string(&Outcome::Error(error.clone()))
        .unwrap_or_else(|_| error.message.clone());
    eprintln!("{line}");
    ExitCode::from(1)
}

fn usage_error(problem: &str) -> ExitCode {
    eprintln!("hive agent: {problem}\n\n{USAGE}");
    ExitCode::from(2)
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Help,
    Call(Invocation),
}

#[derive(Debug, PartialEq)]
struct Invocation {
    request: Request,
    /// How long the caller waits; the app's deadline matches it.
    timeout: Duration,
}

/// Flags and positional words of one `hive agent` command.
#[derive(Default)]
struct Words {
    positional: Vec<String>,
    kind: Option<String>,
    prompt: Option<String>,
    lines: Option<String>,
    timeout: Option<String>,
    tab: bool,
    wait: bool,
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(Parsed::Help);
    };
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        return Ok(Parsed::Help);
    }
    let words = words(rest)?;
    let timeout = |default: Duration| -> Result<Duration, String> {
        match &words.timeout {
            None => Ok(default),
            Some(seconds) => seconds
                .parse::<f64>()
                .ok()
                .filter(|seconds| {
                    seconds.is_finite()
                        && *seconds > 0.0
                        && *seconds <= api::MAX_TIMEOUT.as_secs_f64()
                })
                .map(Duration::from_secs_f64)
                .ok_or_else(|| format!("--timeout expects seconds, got {seconds:?}")),
        }
    };
    let millis = |timeout: Duration| Some(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
    let (request, timeout, allowed): (Request, Duration, &[&str]) = match command.as_str() {
        "list" => {
            positional::<0>(&words)?;
            (Request::List, timeout(QUICK_TIMEOUT)?, &[])
        }
        "start" => {
            let [name] = positional(&words)?;
            if !api::valid_name(&name) {
                return Err(format!(
                    "{name:?} is not a valid name: use lowercase letters, digits, - or _, starting with a letter, at most 32 characters"
                ));
            }
            let kind = kind(words.kind.as_deref())?;
            let timeout = timeout(api::START_TIMEOUT)?;
            let request = Request::Start {
                name,
                kind,
                tab: words.tab,
                prompt: words.prompt.clone(),
                timeout_ms: millis(timeout),
            };
            (request, timeout, &["--kind", "--prompt", "--tab"])
        }
        "send" => {
            let [to, text] = positional(&words)?;
            let timeout = timeout(if words.wait {
                api::WAIT_TIMEOUT
            } else {
                api::SEND_TIMEOUT
            })?;
            let request = Request::Send {
                to: address(&to)?,
                text,
                wait: words.wait,
                timeout_ms: millis(timeout),
            };
            (request, timeout, &["--wait"])
        }
        "wait" => {
            let [target] = positional(&words)?;
            let timeout = timeout(api::WAIT_TIMEOUT)?;
            let request = Request::Wait {
                target: address(&target)?,
                timeout_ms: millis(timeout),
            };
            (request, timeout, &[])
        }
        "read" => {
            let [target] = positional(&words)?;
            let lines = match &words.lines {
                None => None,
                Some(lines) => Some(
                    lines
                        .parse()
                        .map_err(|_| format!("--lines expects a number, got {lines:?}"))?,
                ),
            };
            let request = Request::Read {
                target: address(&target)?,
                lines,
            };
            (request, timeout(QUICK_TIMEOUT)?, &["--lines"])
        }
        "interrupt" => {
            let [target] = positional(&words)?;
            let request = Request::Interrupt {
                target: address(&target)?,
            };
            (request, timeout(QUICK_TIMEOUT)?, &[])
        }
        other => return Err(format!("unknown command {other:?}")),
    };
    reject_unused(&words, allowed)?;
    Ok(Parsed::Call(Invocation { request, timeout }))
}

fn words(args: &[String]) -> Result<Words, String> {
    let mut words = Words::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--" => {
                words.positional.extend(args.by_ref().cloned());
            }
            "--kind" => words.kind = Some(value("--kind")?),
            "--prompt" => words.prompt = Some(value("--prompt")?),
            "--lines" => words.lines = Some(value("--lines")?),
            "--timeout" => words.timeout = Some(value("--timeout")?),
            "--tab" => words.tab = true,
            "--wait" => words.wait = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            word => words.positional.push(word.to_owned()),
        }
    }
    Ok(words)
}

fn positional<const N: usize>(words: &Words) -> Result<[String; N], String> {
    <[String; N]>::try_from(words.positional.clone()).map_err(|got| {
        format!(
            "expected {N} argument{}, got {}",
            if N == 1 { "" } else { "s" },
            got.len()
        )
    })
}

/// Flags that mean nothing for this command are mistakes, not something to ignore silently.
fn reject_unused(words: &Words, allowed: &[&str]) -> Result<(), String> {
    let given = [
        ("--kind", words.kind.is_some()),
        ("--prompt", words.prompt.is_some()),
        ("--lines", words.lines.is_some()),
        ("--tab", words.tab),
        ("--wait", words.wait),
    ];
    match given
        .into_iter()
        .find(|(flag, set)| *set && !allowed.contains(flag))
    {
        Some((flag, _)) => Err(format!("{flag} does not apply to this command")),
        None => Ok(()),
    }
}

fn kind(kind: Option<&str>) -> Result<AgentKind, String> {
    let names = || {
        AGENTS
            .iter()
            .map(|agent| agent.executable())
            .collect::<Vec<_>>()
            .join("|")
    };
    let kind = kind.ok_or_else(|| format!("--kind is required: {}", names()))?;
    AgentKind::from_executable(kind)
        .filter(|agent| AGENTS.contains(agent))
        .ok_or_else(|| format!("--kind must be one of {}, got {kind:?}", names()))
}

fn address(text: &str) -> Result<Address, String> {
    text.parse().map_err(|error: ApiError| error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(line: &[&str]) -> Result<Parsed, String> {
        parse(
            &line
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    fn request(line: &[&str]) -> Invocation {
        match parse_words(line) {
            Ok(Parsed::Call(invocation)) => invocation,
            other => panic!("{line:?} parsed as {other:?}"),
        }
    }

    #[test]
    fn start_takes_a_valid_name_a_launchable_kind_and_an_optional_task() {
        let invocation = request(&[
            "start", "reviewer", "--kind", "codex", "--prompt", "Review", "--tab",
        ]);
        assert_eq!(
            invocation.request,
            Request::Start {
                name: "reviewer".into(),
                kind: AgentKind::Codex,
                tab: true,
                prompt: Some("Review".into()),
                timeout_ms: Some(90_000),
            }
        );
        assert!(parse_words(&["start", "Reviewer", "--kind", "codex"]).is_err());
        assert!(parse_words(&["start", "reviewer"]).is_err());
        assert!(parse_words(&["start", "reviewer", "--kind", "aider"]).is_err());
        assert!(parse_words(&["start", "reviewer", "--kind", "claude", "--wait"]).is_err());
    }

    #[test]
    fn send_waits_longer_only_when_asked_to() {
        let plain = request(&["send", "lead", "done"]);
        assert_eq!(plain.timeout, api::SEND_TIMEOUT);
        let waiting = request(&["send", "12", "--wait", "go", "--timeout", "2.5"]);
        assert_eq!(waiting.timeout, Duration::from_millis(2_500));
        assert_eq!(
            waiting.request,
            Request::Send {
                to: Address::Terminal(12),
                text: "go".into(),
                wait: true,
                timeout_ms: Some(2_500),
            }
        );
        assert!(parse_words(&["send", "lead", "--wait"]).is_err());
    }

    #[test]
    fn a_double_dash_lets_a_message_start_with_dashes() {
        assert_eq!(
            request(&["send", "lead", "--", "--wait is not a flag here"]).request,
            Request::Send {
                to: Address::Name("lead".into()),
                text: "--wait is not a flag here".into(),
                wait: false,
                timeout_ms: Some(60_000),
            }
        );
    }

    #[test]
    fn mistakes_are_usage_errors_not_requests() {
        for line in [
            &["send", "lead"][..],
            &["send", "Lead", "hi"],
            &["read", "lead", "--lines", "many"],
            &["wait", "lead", "--timeout", "0"],
            &["wait", "lead", "--timeout", "-1"],
            &["list", "extra"],
            &["list", "--bogus"],
            &["read"],
            &["frobnicate"],
        ] {
            assert!(parse_words(line).is_err(), "{line:?}");
        }
        assert_eq!(parse_words(&[]), Ok(Parsed::Help));
        assert_eq!(parse_words(&["--help"]), Ok(Parsed::Help));
    }

    #[test]
    fn read_and_wait_name_their_target() {
        assert_eq!(
            request(&["read", "reviewer", "--lines", "40"]).request,
            Request::Read {
                target: Address::Name("reviewer".into()),
                lines: Some(40)
            }
        );
        assert_eq!(
            request(&["wait", "7"]).request,
            Request::Wait {
                target: Address::Terminal(7),
                timeout_ms: Some(600_000)
            }
        );
        assert_eq!(
            request(&["interrupt", "reviewer"]).request,
            Request::Interrupt {
                target: Address::Name("reviewer".into())
            }
        );
        assert!(parse(&["interrupt".into(), "reviewer".into(), "--wait".into()]).is_err());
    }

    #[test]
    fn only_window_less_modes_are_claimed() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(run(&args(&["hive"])).is_none());
        assert!(run(&args(&["hive", "-psn_0_123"])).is_none());
    }
}
