//! Command-line modes of the Hive binary that run without a window: Claude hook forwarding,
//! `hive --skill` and `hive agent …`, which agents in Hive's terminals use to reach the app.
use hive_core::agent::AgentKind;
use hive_core::agent::api::{self, Address, ApiError, Call, ErrorCode, Outcome, Request, Response};
use hive_core::agent::launch::AGENTS;
use std::ffi::{OsStr, OsString};
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
  hive                  Start the Hive app
  hive --help           Show this help
  hive --version        Print the version
  hive --skill          Explain how agents use `hive agent`
  hive agent <command>  Reach the agents in Hive's terminals; `hive agent --help` lists the commands
  hive mcp --socket <absolute-path> --terminal <id>";

/// What a bare `hive` says instead of opening a second window on the same data folder.
const RUNNING: &str = "Hive is already running, and this terminal is one of its panes.";

const AGENT_USAGE: &str = "\
Usage:
  hive agent list
  hive agent report <ticket-json> <result-json>
  hive agent start <name> --kind <claude|codex|opencode|agy> [--prompt <task>] [--tab]
  hive agent send <target> <text> [--wait]
  hive agent wait <target>
  hive agent read <target> [--lines <n>]
  hive agent interrupt <target>
Every command accepts --timeout <seconds>. `hive --skill` explains how agents use them.";

/// How the process was started, where that changes what its arguments mean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Origin {
    /// launchd is the parent: Finder, the Dock, `open` or a relaunch started the app.
    launch_services: bool,
    /// A Hive pane started it, and a running Hive holds the data folder it would use.
    pane_of_running_hive: bool,
}

impl Origin {
    /// Blocking only for a bare `hive` typed in a Hive pane, which asks for the data folder's
    /// lock and gives it back at once.
    pub(crate) fn detect(args: &[OsString]) -> Self {
        let in_pane = || std::env::var_os("HIVE_SOCKET").is_some_and(|socket| !socket.is_empty());
        Self {
            launch_services: std::os::unix::process::parent_id() == 1,
            pane_of_running_hive: args.len() < 2
                && in_pane()
                && !hive_core::state::Access::acquire().can_write(),
        }
    }
}

/// What the arguments after the program name ask the binary to do.
#[derive(Debug, PartialEq)]
enum Mode<'a> {
    /// Open the window.
    App,
    /// A bare `hive` in a pane of the Hive that is already open.
    Running,
    Help,
    Version,
    Skill,
    Hook(&'a [OsString]),
    Mcp(&'a [OsString]),
    Agent(&'a [OsString]),
    /// A word no mode answers to.
    Unknown(&'a OsStr),
}

/// The app starts only when nothing asks for anything else. The `hive` launcher is first on
/// every pane's `PATH`, so a guess such as `hive --help` or `hive status` must never open a
/// second Hive on the same data folder and hold the caller's shell until that window closes.
/// A bare `hive` in a pane of the Hive that is already open gets the usage for the same reason.
///
/// A launch through LaunchServices is the opposite case: `open -a Hive --args …` is deliberate
/// and its stderr leads nowhere, so a word no mode answers to still opens the window. macOS
/// itself adds no arguments; `-psn_…` is the process serial number older versions appended.
fn mode(args: &[OsString], origin: Origin) -> Mode<'_> {
    let Some((first, rest)) = args.get(1..).and_then(<[OsString]>::split_first) else {
        return if origin.pane_of_running_hive && !origin.launch_services {
            Mode::Running
        } else {
            Mode::App
        };
    };
    match first.to_str() {
        Some(serial) if serial.starts_with("-psn_") => Mode::App,
        Some("--help" | "-h" | "help") => Mode::Help,
        Some("--version" | "-V") => Mode::Version,
        Some("--skill") => Mode::Skill,
        Some("--agent-hook") => Mode::Hook(rest),
        Some("mcp") => Mode::Mcp(rest),
        Some("agent") => Mode::Agent(rest),
        _ if origin.launch_services => Mode::App,
        _ => Mode::Unknown(first),
    }
}

/// Runs a window-less mode named by `args`, or returns `None` to start the app.
pub(crate) fn run(args: &[OsString], origin: Origin) -> Option<ExitCode> {
    Some(match mode(args, origin) {
        Mode::App => return None,
        Mode::Running => print(&format!("{RUNNING}\n\n{USAGE}")),
        Mode::Help => print(USAGE),
        Mode::Version => print(concat!("hive ", env!("CARGO_PKG_VERSION"))),
        Mode::Skill => print(api::SKILL.trim_end()),
        Mode::Hook(args) => forward_hook(args),
        Mode::Mcp(args) => crate::mcp::run(args),
        Mode::Agent(args) => agent(args),
        Mode::Unknown(word) => {
            eprintln!("hive: unknown argument {word:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    })
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
        Some(Ok(Parsed::Help)) => return print(AGENT_USAGE),
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
fn call(mut invocation: Invocation) -> Result<Response, ApiError> {
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
    if let Request::Report { grant, .. } = &mut invocation.request {
        *grant = Some(api::ReportGrant::inherited().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidRequest,
                "Pipeline reporting is only available in the assigned pane",
            )
        })?);
    }
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
    eprintln!("hive agent: {problem}\n\n{AGENT_USAGE}");
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
        "report" => {
            let [ticket, result] = positional(&words)?;
            let ticket =
                serde_json::from_str(&ticket).map_err(|_| "Invalid pipeline ticket JSON")?;
            let result =
                serde_json::from_str(&result).map_err(|_| "Invalid pipeline result JSON")?;
            (
                Request::Report {
                    ticket,
                    result,
                    grant: None,
                },
                timeout(QUICK_TIMEOUT)?,
                &[],
            )
        }
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
    fn report_accepts_only_a_ticket_and_typed_result_and_never_takes_a_grant_argument() {
        let args = [
            "report",
            r#"{"run":4,"attempt":5,"lane":null}"#,
            r#"{"kind":"developed","summary":"done"}"#,
        ];
        assert!(matches!(
            request(&args).request,
            Request::Report { grant: None, .. }
        ));
        assert!(parse_words(&["report", args[1], r#"{"kind":"unexpected"}"#]).is_err());
        assert!(parse_words(&["report", args[1], args[2], "forged"]).is_err());
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

    fn line(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    /// A command typed in a terminal that is not a pane of a running Hive.
    const SHELL: Origin = Origin {
        launch_services: false,
        pane_of_running_hive: false,
    };
    /// Finder, the Dock or `open`.
    const LAUNCH_SERVICES: Origin = Origin {
        launch_services: true,
        pane_of_running_hive: false,
    };
    /// The `hive` launcher in a pane of the Hive that holds the data folder.
    const PANE: Origin = Origin {
        launch_services: false,
        pane_of_running_hive: true,
    };

    fn from_shell(args: &[OsString]) -> Mode<'_> {
        mode(args, SHELL)
    }

    #[test]
    fn a_launch_through_launch_services_opens_the_app_whatever_follows_args() {
        // `open -a Hive --args …`: deliberate, and its stderr leads nowhere.
        for words in [
            &["hive", "-AppleLanguages", "(en)"][..],
            &["hive", "-NSDocumentRevisionsDebugMode", "YES"],
            &["hive", "status"],
        ] {
            assert_eq!(mode(&line(words), LAUNCH_SERVICES), Mode::App, "{words:?}");
        }
        assert_eq!(mode(&line(&["hive"]), LAUNCH_SERVICES), Mode::App);
        // What Hive itself answers to stays a mode: an orphaned `hive mcp` is not a launch.
        let mcp = line(&["hive", "mcp", "--socket", "/tmp/hh/a", "--terminal", "7"]);
        assert_eq!(mode(&mcp, LAUNCH_SERVICES), Mode::Mcp(&mcp[2..]));
        assert_eq!(
            mode(&line(&["hive", "--version"]), LAUNCH_SERVICES),
            Mode::Version
        );
    }

    #[test]
    fn a_bare_hive_in_a_pane_of_the_running_app_never_opens_a_second_window() {
        assert_eq!(mode(&line(&["hive"]), PANE), Mode::Running);
        assert_eq!(run(&line(&["hive"]), SHELL), None);
        // `open -n -a Hive` from a pane passes the pane's environment on, and asks for a window.
        let opened_from_pane = Origin {
            launch_services: true,
            pane_of_running_hive: true,
        };
        assert_eq!(mode(&line(&["hive"]), opened_from_pane), Mode::App);
        // In a pane everything else means what it means anywhere.
        assert_eq!(mode(&line(&["hive", "--help"]), PANE), Mode::Help);
        let unknown = line(&["hive", "status"]);
        assert_eq!(mode(&unknown, PANE), Mode::Unknown(&unknown[1]));
    }

    #[test]
    fn the_app_starts_only_without_arguments_or_with_a_process_serial_number() {
        assert_eq!(from_shell(&line(&["hive"])), Mode::App);
        assert_eq!(from_shell(&line(&["hive", "-psn_0_123"])), Mode::App);
        // A caller may exec the binary without even a program name.
        assert_eq!(from_shell(&[]), Mode::App);
        assert!(run(&line(&["hive"]), SHELL).is_none());
        assert!(run(&line(&["hive", "-psn_0_123"]), SHELL).is_none());
    }

    #[test]
    fn help_and_version_are_answered_without_a_window() {
        for flag in ["--help", "-h", "help"] {
            assert_eq!(from_shell(&line(&["hive", flag])), Mode::Help, "{flag}");
        }
        for flag in ["--version", "-V"] {
            assert_eq!(from_shell(&line(&["hive", flag])), Mode::Version, "{flag}");
        }
        assert_eq!(from_shell(&line(&["hive", "--help", "agent"])), Mode::Help);
    }

    #[test]
    fn a_window_less_mode_gets_the_words_after_its_name() {
        let agent = line(&["hive", "agent", "send", "lead", "hi"]);
        assert_eq!(from_shell(&agent), Mode::Agent(&agent[2..]));
        assert_eq!(from_shell(&line(&["hive", "agent"])), Mode::Agent(&[]));
        let mcp = line(&["hive", "mcp", "--socket", "/tmp/hh/a", "--terminal", "7"]);
        assert_eq!(from_shell(&mcp), Mode::Mcp(&mcp[2..]));
        let hook = line(&["hive", "--agent-hook", "/tmp/hh/e", "7"]);
        assert_eq!(from_shell(&hook), Mode::Hook(&hook[2..]));
        assert_eq!(from_shell(&line(&["hive", "--skill"])), Mode::Skill);
    }

    #[test]
    fn an_unknown_argument_is_a_usage_error_and_never_starts_the_app() {
        for word in [
            "status",
            "--verbose",
            "--helpful",
            "Agent",
            "-psn",
            "",
            "-NSDocumentRevisionsDebugMode",
        ] {
            let args = line(&["hive", word]);
            assert_eq!(
                from_shell(&args),
                Mode::Unknown(OsStr::new(word)),
                "{word:?}"
            );
        }
        use std::os::unix::ffi::OsStringExt as _;
        let not_utf8 = [OsString::from("hive"), OsString::from_vec(vec![0xff, 0xfe])];
        assert_eq!(from_shell(&not_utf8), Mode::Unknown(&not_utf8[1]));
        assert_eq!(
            run(&line(&["hive", "status"]), SHELL),
            Some(ExitCode::from(2)),
            "a usage error, as for `hive agent`"
        );
    }
}
