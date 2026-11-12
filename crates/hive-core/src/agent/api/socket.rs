//! The private Unix socket behind `hive agent`: one JSON line in and one out per connection.
//! Every connection gets its own thread, so a long `wait` never holds up another call.
use super::{ApiError, Call, ErrorCode, Outcome};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A call carries at most one message; anything larger is not a Hive call.
const MAX_REQUEST: usize = 64 * 1024;
/// `read` answers with up to a couple of thousand terminal lines.
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
/// A client that connects but never finishes its line must not pin a thread.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Calls served at once. Each holds a thread until the app answers, so a process that opens
/// connections in a loop must not exhaust them; the panes of one window make a handful.
const MAX_CALLS: usize = 64;

/// One served call; dropping it frees its place under [`MAX_CALLS`].
struct Served(Arc<AtomicUsize>);

impl Served {
    fn begin(live: &Arc<AtomicUsize>) -> Option<Self> {
        if live.fetch_add(1, Ordering::AcqRel) >= MAX_CALLS {
            live.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self(live.clone()))
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A call waiting for the app's answer. Dropping it unanswered tells the caller the app
/// is unavailable.
pub struct Incoming {
    call: Call,
    reply: async_channel::Sender<Outcome>,
}

impl Incoming {
    /// What the caller asked.
    pub fn call(&self) -> &Call {
        &self.call
    }

    /// Answers the caller; one that already hung up is ignored.
    pub fn reply(self, outcome: Outcome) {
        let _ = self.reply.try_send(outcome);
    }
}

/// Blocking: listen at `path` and hand every call to the returned receiver.
///
/// The socket should live in an owner-only directory: whoever can connect can type into
/// the user's agents.
pub fn listen(path: &Path) -> io::Result<async_channel::Receiver<Incoming>> {
    let listener = UnixListener::bind(path)?;
    let (calls, receiver) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("hive-agent-api".into())
        .spawn(move || accept(&listener, &calls))?;
    Ok(receiver)
}

/// Blocking: send one call and wait up to `timeout` for its answer.
pub fn call(socket: &Path, call: &Call, timeout: Duration) -> io::Result<Outcome> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let mut line = serde_json::to_vec(call).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let response = read_line(&stream, MAX_RESPONSE)?;
    serde_json::from_slice(&response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

// ponytail: the accept thread ends only with the process; it notices a dropped receiver at
// the next connection. Add a shutdown handshake if Hive ever rebinds the socket at runtime.
fn accept(listener: &UnixListener, calls: &async_channel::Sender<Incoming>) {
    let live = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        if calls.is_closed() {
            return;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "agent API connection failed");
                continue;
            }
        };
        let Some(served) = Served::begin(&live) else {
            refuse(&stream);
            continue;
        };
        let calls = calls.clone();
        let spawned = std::thread::Builder::new()
            .name("hive-agent-call".into())
            .spawn(move || {
                let _served = served;
                if let Err(error) = serve(&stream, &calls) {
                    tracing::debug!(%error, "agent API caller went away");
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not serve an agent API call");
        }
    }
}

fn serve(stream: &UnixStream, calls: &async_channel::Sender<Incoming>) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let outcome = match read_line(stream, MAX_REQUEST)
        .and_then(|line| serde_json::from_slice::<Call>(&line).map_err(io::Error::other))
    {
        Ok(call) => dispatch(call, calls),
        Err(error) => Outcome::Error(ApiError::new(
            ErrorCode::InvalidRequest,
            format!("Not a Hive agent call: {error}"),
        )),
    };
    reply(stream, &outcome)
}

/// Answers a call there is no thread for. The line fits the socket buffer, so the accept
/// loop does not wait for the caller to read it.
fn refuse(stream: &UnixStream) {
    let busy = Outcome::Error(ApiError::new(
        ErrorCode::Unavailable,
        "Hive is serving too many agent calls; try again shortly",
    ));
    if let Err(error) = stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| reply(stream, &busy))
    {
        tracing::debug!(%error, "agent API caller went away");
    }
}

fn reply(stream: &UnixStream, outcome: &Outcome) -> io::Result<()> {
    let mut line = serde_json::to_vec(outcome).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut writer = stream;
    writer.write_all(&line)
}

/// Blocks this connection's thread until the app answers.
fn dispatch(call: Call, calls: &async_channel::Sender<Incoming>) -> Outcome {
    let unavailable = || {
        Outcome::Error(ApiError::new(
            ErrorCode::Unavailable,
            "Hive dropped the call: the pane closed or Hive is quitting",
        ))
    };
    let (reply, answer) = async_channel::bounded(1);
    if calls.send_blocking(Incoming { call, reply }).is_err() {
        return unavailable();
    }
    answer.recv_blocking().unwrap_or_else(|_| unavailable())
}

/// One newline-terminated line, without the newline; EOF also ends it.
fn read_line(stream: &UnixStream, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    BufReader::new(stream.take(limit as u64 + 1)).read_until(b'\n', &mut line)?;
    if line.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line exceeds {limit} bytes"),
        ));
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::{Request, Response};

    /// A socket path in a fresh per-test directory; Unix socket paths must stay short.
    fn socket(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("a")
    }

    #[test]
    fn a_call_reaches_the_app_and_its_answer_reaches_the_caller() {
        let path = socket("answer");
        let calls = listen(&path).unwrap();
        let app = std::thread::spawn(move || {
            let incoming = calls.recv_blocking().unwrap();
            assert_eq!(incoming.call().from, Some(9));
            assert_eq!(incoming.call().request, Request::List);
            incoming.reply(Outcome::Result(Response::Agents { agents: Vec::new() }));
        });
        let outcome = call(
            &path,
            &Call {
                from: Some(9),
                request: Request::List,
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            outcome,
            Outcome::Result(Response::Agents { agents: Vec::new() })
        );
        app.join().unwrap();
    }

    #[test]
    fn a_dropped_call_answers_unavailable() {
        let path = socket("dropped");
        let calls = listen(&path).unwrap();
        let app = std::thread::spawn(move || drop(calls.recv_blocking().unwrap()));
        let outcome = call(
            &path,
            &Call {
                from: None,
                request: Request::List,
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            Outcome::Error(ApiError {
                code: ErrorCode::Unavailable,
                ..
            })
        ));
        app.join().unwrap();
    }

    #[test]
    fn malformed_and_oversized_lines_are_refused_without_reaching_the_app() {
        let path = socket("malformed");
        let calls = listen(&path).unwrap();
        for line in [b"not json\n".to_vec(), vec![b'x'; MAX_REQUEST + 10]] {
            let mut stream = UnixStream::connect(&path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream.write_all(&line).unwrap();
            let answer = read_line(&stream, MAX_RESPONSE).unwrap();
            let outcome: Outcome = serde_json::from_slice(&answer).unwrap();
            assert!(matches!(
                outcome,
                Outcome::Error(ApiError {
                    code: ErrorCode::InvalidRequest,
                    ..
                })
            ));
        }
        assert!(calls.try_recv().is_err());
    }

    #[test]
    fn calls_beyond_the_limit_are_refused_without_reaching_the_app() {
        let path = socket("limit");
        let calls = listen(&path).unwrap();
        let list = Call {
            from: None,
            request: Request::List,
        };
        let mut line = serde_json::to_vec(&list).unwrap();
        line.push(b'\n');
        let mut callers = Vec::new();
        let mut held = Vec::new();
        for _ in 0..MAX_CALLS {
            let mut stream = UnixStream::connect(&path).unwrap();
            stream.write_all(&line).unwrap();
            callers.push(stream);
            held.push(calls.recv_blocking().unwrap());
        }
        let outcome = call(&path, &list, Duration::from_secs(5)).unwrap();
        assert!(matches!(
            outcome,
            Outcome::Error(ApiError {
                code: ErrorCode::Unavailable,
                ..
            })
        ));
        assert!(calls.try_recv().is_err());
    }

    #[test]
    fn a_finished_call_frees_its_place_for_the_next() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut served: Vec<_> = (0..MAX_CALLS)
            .map(|_| Served::begin(&live).unwrap())
            .collect();
        assert!(Served::begin(&live).is_none());
        assert_eq!(live.load(Ordering::Acquire), MAX_CALLS);
        served.pop();
        assert!(Served::begin(&live).is_some());
        assert_eq!(live.load(Ordering::Acquire), MAX_CALLS - 1);
    }

    #[test]
    fn a_silent_app_times_the_caller_out() {
        let path = socket("silent");
        let _calls = listen(&path).unwrap();
        let error = call(
            &path,
            &Call {
                from: None,
                request: Request::List,
            },
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn no_listener_is_a_connection_error() {
        let path = socket("missing");
        assert!(
            call(
                &path,
                &Call {
                    from: None,
                    request: Request::List
                },
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
