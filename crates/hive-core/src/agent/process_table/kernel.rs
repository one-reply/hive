//! The process table read from the macOS kernel with `sysctl`, without spawning `ps`.
//!
//! The agents poll takes a snapshot every 800 ms; a `ps` child per snapshot cost about 20 ms
//! of CPU each time. `KERN_PROC_ALL` answers for every process whatever its owner, which the
//! per-process `proc_pidinfo` does not: it refuses other users' processes, and every terminal
//! shell hangs under root's `login`.
use super::Process;
use std::io;

/// `struct kinfo_proc` from `<sys/sysctl.h>` on 64-bit macOS. The `libc` crate does not
/// declare it, so the few fields read here are addressed by offset. The live-snapshot tests
/// pin these against the running kernel.
const RECORD_SIZE: usize = 648;
/// `kp_proc.p_pid`.
const PID_OFFSET: usize = 40;
/// `kp_proc.p_comm`, `MAXCOMLEN + 1` bytes, NUL-terminated.
const NAME_OFFSET: usize = 243;
const NAME_SIZE: usize = 17;
/// `kp_eproc.e_pgid`.
const GROUP_OFFSET: usize = 564;
/// `kp_eproc.e_tpgid`: the foreground group of the controlling terminal, 0 without one.
const FOREGROUND_GROUP_OFFSET: usize = 576;
/// Processes can start between sizing the list and reading it.
const LIST_ATTEMPTS: usize = 3;
const LIST_HEADROOM: usize = 32 * RECORD_SIZE;
/// The kernel never returns more arguments than this when its own limit cannot be read.
const DEFAULT_ARGUMENTS_SIZE: usize = 1024 * 1024;

/// One `kinfo_proc`, reduced to what the table keeps.
#[derive(Debug, PartialEq, Eq)]
struct Record {
    pid: u32,
    pgid: u32,
    tpgid: i32,
    /// The executable's name, cut to 16 bytes by the kernel.
    name: String,
}

/// Blocking: every local process. Command lines are read only for processes with a
/// controlling terminal, the only ones a foreground job can contain; the rest, and processes
/// whose arguments the kernel keeps from us (another user's, zombies), carry their name.
pub(super) fn processes() -> io::Result<Vec<Process>> {
    let records = parse_process_list(&process_list()?)?;
    // One buffer for the whole snapshot. It must hold a full argument area: the kernel
    // answers a shorter buffer with zeros instead of an error.
    let mut arguments = vec![0; arguments_size()];
    Ok(records
        .into_iter()
        .map(|record| {
            let args = (record.tpgid > 0)
                .then(|| read_arguments(record.pid, &mut arguments))
                .flatten()
                .unwrap_or(record.name);
            Process {
                pid: record.pid,
                pgid: record.pgid,
                tpgid: i64::from(record.tpgid),
                args,
            }
        })
        .collect())
}

fn process_list() -> io::Result<Vec<u8>> {
    let mut name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
    let mut last_error = io::Error::from_raw_os_error(libc::ENOMEM);
    for _ in 0..LIST_ATTEMPTS {
        let mut list = vec![0; sysctl_size(&mut name)? + LIST_HEADROOM];
        match read_sysctl(&mut name, &mut list) {
            Ok(written) => {
                list.truncate(written);
                return Ok(list);
            }
            // The list outgrew the buffer since it was sized.
            Err(error) if error.raw_os_error() == Some(libc::ENOMEM) => last_error = error,
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

fn parse_process_list(list: &[u8]) -> io::Result<Vec<Record>> {
    let (records, rest) = list.as_chunks::<RECORD_SIZE>();
    if !rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "process list of {} bytes is not made of {RECORD_SIZE}-byte records",
                list.len()
            ),
        ));
    }
    Ok(records.iter().filter_map(record).collect())
}

/// `None` for the kernel's own pid 0, which `ps` does not list either.
fn record(bytes: &[u8; RECORD_SIZE]) -> Option<Record> {
    let pid = u32::try_from(int_at(bytes, PID_OFFSET)?)
        .ok()
        .filter(|&pid| pid > 0)?;
    let name = bytes.get(NAME_OFFSET..NAME_OFFSET + NAME_SIZE)?;
    let name = name.split(|&byte| byte == 0).next().unwrap_or_default();
    Some(Record {
        pid,
        pgid: u32::try_from(int_at(bytes, GROUP_OFFSET)?).ok()?,
        tpgid: int_at(bytes, FOREGROUND_GROUP_OFFSET)?,
        name: String::from_utf8_lossy(name).into_owned(),
    })
}

fn int_at(bytes: &[u8], offset: usize) -> Option<i32> {
    let field = bytes.get(offset..offset + size_of::<i32>())?;
    Some(i32::from_ne_bytes(field.try_into().ok()?))
}

fn arguments_size() -> usize {
    let mut limit = [0; size_of::<libc::c_int>()];
    read_sysctl(&mut [libc::CTL_KERN, libc::KERN_ARGMAX], &mut limit)
        .ok()
        .filter(|&written| written == limit.len())
        .and_then(|_| usize::try_from(libc::c_int::from_ne_bytes(limit)).ok())
        .filter(|&size| size > 0)
        .unwrap_or(DEFAULT_ARGUMENTS_SIZE)
}

fn read_arguments(pid: u32, buffer: &mut [u8]) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let written = read_sysctl(&mut [libc::CTL_KERN, libc::KERN_PROCARGS2, pid], buffer).ok()?;
    parse_arguments(buffer.get(..written)?)
}

/// A `KERN_PROCARGS2` area as `ps -o args=` prints it: the arguments joined by single spaces.
/// The area holds the argument count, the executable path, NUL padding, then each argument
/// NUL-terminated. `None` when it carries no argument.
fn parse_arguments(area: &[u8]) -> Option<String> {
    let (count, rest) = area.split_first_chunk::<{ size_of::<i32>() }>()?;
    let count = usize::try_from(i32::from_ne_bytes(*count)).ok()?;
    let path_end = rest.iter().position(|&byte| byte == 0)?;
    let padding = rest[path_end..]
        .iter()
        .take_while(|&&byte| byte == 0)
        .count();
    let arguments: Vec<_> = rest[path_end + padding..]
        .split(|&byte| byte == 0)
        .take(count)
        .map(String::from_utf8_lossy)
        .collect();
    // A CLI that rewrites its title leaves the arguments it erased empty.
    let line = arguments.join(" ");
    let line = line.trim_end();
    (!line.is_empty()).then(|| line.to_owned())
}

/// The size the kernel would write for `name`.
fn sysctl_size(name: &mut [libc::c_int]) -> io::Result<usize> {
    let mut size = 0;
    // SAFETY: without an output buffer the kernel only stores the needed size in `size`.
    let status = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status == 0 {
        Ok(size)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Reads the value of `name` into `buffer` and returns how many bytes the kernel wrote.
fn read_sysctl(name: &mut [libc::c_int], buffer: &mut [u8]) -> io::Result<usize> {
    let mut size = buffer.len();
    // SAFETY: `size` tells the kernel the capacity of `buffer`; it writes at most that many
    // bytes and stores the count back.
    let status = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status == 0 {
        Ok(size)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;

    /// Captured from `/bin/sleep 30` on macOS 27.2.
    const SLEEP: &[u8] = b"\x02\x00\x00\x00/bin/sleep\x00\x00\x00\x00\x00\x00/bin/sleep\x0030\x00";
    /// Captured from a process whose `argv[0]` is a path with a space.
    const SPACED: &[u8] = b"\x02\x00\x00\x00/bin/sleep\x00\x00\x00\x00\x00\x00/Users/me/Library/Application Support/Claude/claude\x0030\x00";

    fn list_record(pid: i32, pgid: i32, tpgid: i32, name: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; RECORD_SIZE];
        bytes[PID_OFFSET..PID_OFFSET + 4].copy_from_slice(&pid.to_ne_bytes());
        bytes[GROUP_OFFSET..GROUP_OFFSET + 4].copy_from_slice(&pgid.to_ne_bytes());
        bytes[FOREGROUND_GROUP_OFFSET..FOREGROUND_GROUP_OFFSET + 4]
            .copy_from_slice(&tpgid.to_ne_bytes());
        bytes[NAME_OFFSET..NAME_OFFSET + name.len()].copy_from_slice(name);
        bytes
    }

    #[test]
    fn arguments_are_joined_with_single_spaces() {
        assert_eq!(parse_arguments(SLEEP).as_deref(), Some("/bin/sleep 30"));
    }

    #[test]
    fn executable_path_with_a_space_still_names_its_agent() {
        let line = parse_arguments(SPACED).unwrap();
        assert_eq!(
            line,
            "/Users/me/Library/Application Support/Claude/claude 30"
        );
        assert_eq!(AgentKind::from_command(&line), Some(AgentKind::ClaudeCode));
    }

    #[test]
    fn arguments_stop_at_the_count_and_ignore_what_follows() {
        let mut area = SLEEP.to_vec();
        area.extend_from_slice(b"PATH=/usr/bin\0HOME=/Users/me\0");
        assert_eq!(parse_arguments(&area).as_deref(), Some("/bin/sleep 30"));
    }

    #[test]
    fn erased_trailing_arguments_are_dropped() {
        let area = b"\x03\x00\x00\x00/usr/bin/node\x00\x00\x00claude\x00\x00\x00";
        assert_eq!(parse_arguments(area).as_deref(), Some("claude"));
    }

    #[test]
    fn truncated_or_empty_area_has_no_arguments() {
        assert_eq!(parse_arguments(b""), None);
        assert_eq!(parse_arguments(b"\x02\x00"), None);
        assert_eq!(parse_arguments(b"\x02\x00\x00\x00/bin/sleep"), None);
        assert_eq!(parse_arguments(b"\x02\x00\x00\x00/bin/sleep\x00\x00"), None);
        assert_eq!(
            parse_arguments(b"\xff\xff\xff\xff/bin/sleep\x00a\x00"),
            None
        );
        // The kernel answers a buffer that is too short with the count and zeros.
        assert_eq!(parse_arguments(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn process_list_is_read_record_by_record() {
        let mut list = list_record(51000, 51000, 98260, b"zsh");
        list.extend(list_record(98260, 98260, 98260, b"claude"));
        list.extend(list_record(18450, 18450, 0, b"codex"));
        let records = parse_process_list(&list).unwrap();
        assert_eq!(
            records,
            [
                (51000, 51000, 98260, "zsh"),
                (98260, 98260, 98260, "claude"),
                (18450, 18450, 0, "codex")
            ]
            .map(|(pid, pgid, tpgid, name)| Record {
                pid,
                pgid,
                tpgid,
                name: name.to_owned(),
            })
        );
    }

    #[test]
    fn name_filling_its_whole_field_is_kept() {
        let list = list_record(7, 7, 0, b"sixteen-byte-nam\0");
        assert_eq!(
            parse_process_list(&list).unwrap()[0].name,
            "sixteen-byte-nam"
        );
    }

    #[test]
    fn kernel_task_is_skipped() {
        let mut list = list_record(0, 0, 0, b"kernel_task");
        list.extend(list_record(1, 1, 0, b"launchd"));
        let pids: Vec<_> = parse_process_list(&list)
            .unwrap()
            .into_iter()
            .map(|record| record.pid)
            .collect();
        assert_eq!(pids, [1]);
    }

    #[test]
    fn process_list_that_is_not_whole_records_is_rejected() {
        let mut list = list_record(1, 1, 0, b"launchd");
        list.pop();
        let error = parse_process_list(&list).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(parse_process_list(&[]).unwrap().is_empty());
    }

    #[test]
    fn snapshot_lists_this_process_with_its_group() {
        let processes = processes().unwrap();
        let own = std::process::id();
        let this = processes.iter().find(|p| p.pid == own).unwrap();
        let group = rustix::process::getpgid(None).unwrap();
        assert_eq!(this.pgid, group.as_raw_nonzero().get() as u32);
    }

    #[test]
    fn snapshot_lists_another_users_process_by_name() {
        let processes = processes().unwrap();
        let init = processes.iter().find(|p| p.pid == 1).unwrap();
        assert_eq!(
            (init.pgid, init.tpgid, init.args.as_str()),
            (1, 0, "launchd")
        );
    }

    #[test]
    fn arguments_of_a_running_child_are_read() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut buffer = vec![0; arguments_size()];
        let arguments = read_arguments(child.id(), &mut buffer);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(arguments.as_deref(), Some("/bin/sleep 30"));
    }
}
