use super::{Error, wire};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

/// Input the server has not taken may grow to this before the session ends: room for several
/// of the largest frames, since an editor buffer is at most 1 MiB.
const BACKLOG_LIMIT: usize = 4 * wire::FRAME_LIMIT;
/// How long input may wait while the server neither reads it nor sends anything.
const STALL: Duration = Duration::from_secs(5);
/// Why a session ends on input the server does not take.
const NOT_READING: &str = "server stopped reading its input";

/// The server's stdin with the frames its pipe could not take yet, in order. Writes never
/// block: the worker that writes also reads the server's output, and a server that writes a
/// large answer before it reads again would otherwise wait on the worker while the worker
/// waits on it. Callers pass the clock, so the stall rules are testable.
pub(super) struct Input {
    pipe: File,
    /// Encoded frames, oldest first; the first may be partly written.
    frames: VecDeque<Vec<u8>>,
    /// Bytes of the first frame already written.
    written: usize,
    /// Bytes in `frames` not yet written.
    pending: usize,
    /// Since when queued input has waited with the server neither reading nor sending.
    waiting_since: Instant,
}

impl Input {
    /// `pipe` is the write end of the server's stdin, already non-blocking.
    pub(super) fn new(pipe: impl Into<OwnedFd>, now: Instant) -> Self {
        Self {
            pipe: File::from(pipe.into()),
            frames: VecDeque::new(),
            written: 0,
            pending: 0,
            waiting_since: now,
        }
    }

    /// Queues `frame` behind earlier input and writes what the pipe takes now.
    pub(super) fn push(&mut self, frame: Vec<u8>, now: Instant) -> Result<(), Error> {
        self.flush(now)?;
        if self.pending + frame.len() > BACKLOG_LIMIT {
            return Err(Error::Unavailable(NOT_READING));
        }
        if self.frames.is_empty() {
            self.waiting_since = now;
        }
        self.pending += frame.len();
        self.frames.push_back(frame);
        self.flush(now)
    }

    /// Writes queued input until the pipe is full or the queue is empty.
    pub(super) fn flush(&mut self, now: Instant) -> Result<(), Error> {
        while let Some(frame) = self.frames.front() {
            let length = frame.len();
            match self.pipe.write(&frame[self.written..]) {
                Ok(0) => return Err(Error::Unavailable("server closed stdin")),
                Ok(count) => {
                    self.written += count;
                    self.pending -= count;
                    self.waiting_since = now;
                    if self.written == length {
                        self.frames.pop_front();
                        self.written = 0;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(Error::Protocol(e.to_string())),
            }
        }
        Ok(())
    }

    /// Writes all queued input, waiting for the pipe until `deadline`.
    pub(super) fn flush_until(&mut self, deadline: Instant) -> Result<(), Error> {
        loop {
            let now = Instant::now();
            self.flush(now)?;
            if self.frames.is_empty() {
                return Ok(());
            }
            if now >= deadline {
                return Err(Error::TimedOut);
            }
            let timeout = Timespec::try_from(Duration::from_millis(25))
                .map_err(|e| Error::Protocol(e.to_string()))?;
            // A signal only ends this wait early; the loop checks the deadline again.
            match poll(
                &mut [PollFd::new(&self.pipe, PollFlags::OUT)],
                Some(&timeout),
            ) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(Error::Protocol(error.to_string())),
            }
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        !self.frames.is_empty()
    }

    /// The server sent something: it is alive and reads again once it has written.
    pub(super) fn heard_from_server(&mut self, now: Instant) {
        self.waiting_since = now;
    }

    /// Fails once queued input has waited [`STALL`] with the server silent.
    pub(super) fn check_stalled(&self, now: Instant) -> Result<(), Error> {
        if self.is_pending() && now.saturating_duration_since(self.waiting_since) >= STALL {
            return Err(Error::Unavailable(NOT_READING));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    use std::io::{PipeReader, Read};

    /// More than a pipe buffer holds.
    const LARGE: usize = 1024 * 1024;

    fn pipe(now: Instant) -> (Input, PipeReader) {
        let (reader, writer) = std::io::pipe().unwrap();
        fcntl_setfl(&writer, fcntl_getfl(&writer).unwrap() | OFlags::NONBLOCK).unwrap();
        (Input::new(writer, now), reader)
    }

    /// A pipe the server stopped reading after it had taken everything queued.
    fn full_pipe(now: Instant) -> (Input, PipeReader) {
        let (reader, writer) = std::io::pipe().unwrap();
        fcntl_setfl(&writer, fcntl_getfl(&writer).unwrap() | OFlags::NONBLOCK).unwrap();
        let mut filler = writer.try_clone().unwrap();
        while filler.write(&[0; 4096]).is_ok() {}
        (Input::new(writer, now), reader)
    }

    fn stalled(input: &Input, now: Instant) -> bool {
        input.check_stalled(now).is_err()
    }

    #[test]
    fn frames_the_pipe_could_not_take_reach_the_server_whole_and_in_order() {
        let start = Instant::now();
        let (mut input, mut reader) = pipe(start);
        let frames: Vec<Vec<u8>> = (b'a'..=b'c').map(|byte| vec![byte; LARGE]).collect();
        for frame in &frames {
            input.push(frame.clone(), start).unwrap();
        }
        assert!(input.is_pending(), "a pipe holds less than one frame");
        let server = std::thread::spawn(move || {
            let mut received = Vec::new();
            reader.read_to_end(&mut received).unwrap();
            received
        });
        input
            .flush_until(Instant::now() + Duration::from_secs(30))
            .unwrap();
        drop(input);
        assert_eq!(server.join().unwrap(), frames.concat());
    }

    #[test]
    fn input_waiting_on_a_silent_server_stalls_and_a_server_that_speaks_does_not() {
        let start = Instant::now();
        let (mut input, _reader) = full_pipe(start);
        // Nothing queued: a quiet server is idle, not stuck.
        assert!(!stalled(&input, start + STALL * 10));
        // The wait starts when input is queued, not when the server last read.
        let queued = start + STALL * 2;
        input.push(b"frame".to_vec(), queued).unwrap();
        assert!(input.is_pending());
        assert!(!stalled(&input, queued + STALL - Duration::from_millis(1)));
        assert!(stalled(&input, queued + STALL));
        // A server that writes is alive and reads again afterwards.
        let spoke = queued + STALL;
        input.heard_from_server(spoke);
        assert!(!stalled(&input, spoke + STALL - Duration::from_millis(1)));
        assert!(stalled(&input, spoke + STALL));
        // A clock read before the server last spoke does not count as waiting.
        assert!(!stalled(&input, start));
    }

    #[test]
    fn a_closed_server_pipe_is_an_error_not_a_wait() {
        let start = Instant::now();
        let (mut input, reader) = pipe(start);
        drop(reader);
        assert!(input.push(b"frame".to_vec(), start).is_err());
    }
}
