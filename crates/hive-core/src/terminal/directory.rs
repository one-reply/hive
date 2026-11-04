//! macOS exposes a shell's current directory without changing its startup files.
use super::TerminalError;
use std::{ffi::OsString, mem::MaybeUninit, os::unix::ffi::OsStringExt, path::PathBuf};

/// Blocking: read the live shell's working directory. Call only while its terminal is alive.
pub fn working_directory(shell_pid: u32) -> Result<PathBuf, TerminalError> {
    match process_directory(shell_pid) {
        Ok(path) => Ok(path),
        Err(error) => {
            // macOS login stays as a protected parent; its sole child is the interactive shell.
            let mut children = [0i32; 2];
            let count = unsafe {
                libc::proc_listchildpids(
                    shell_pid as i32,
                    children.as_mut_ptr().cast(),
                    std::mem::size_of_val(&children) as i32,
                )
            };
            if count == 1 && children[0] > 0 {
                process_directory(children[0] as u32)
            } else {
                Err(error)
            }
        }
    }
}

fn process_directory(shell_pid: u32) -> Result<PathBuf, TerminalError> {
    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    // The kernel initializes this fixed-size structure; inspect it only after a complete read.
    let read = unsafe {
        libc::proc_pidinfo(
            shell_pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return Err(TerminalError::Directory {
            pid: shell_pid,
            source: std::io::Error::last_os_error(),
        });
    }
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .into_iter()
        .flatten()
        .take_while(|b| *b != 0)
        .map(|b| b as u8)
        .collect();
    let path = PathBuf::from(OsString::from_vec(bytes));
    if !path.is_absolute() {
        return Err(TerminalError::Directory {
            pid: shell_pid,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "shell returned no absolute directory",
            ),
        });
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_current_process_directory_and_rejects_missing_process() {
        assert_eq!(
            working_directory(std::process::id()).unwrap(),
            std::env::current_dir().unwrap()
        );
        assert!(working_directory(i32::MAX as u32).is_err());
    }
}
