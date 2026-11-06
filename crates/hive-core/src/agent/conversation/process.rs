//! macOS `libproc` queries about another process: its open files and its start time.
use std::{
    ffi::OsString,
    mem::MaybeUninit,
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Agents keep a few dozen descriptors open; the cap bounds work for a misbehaving process.
const OPEN_FILES_LIMIT: usize = 4096;
/// `PROC_PIDFDVNODEPATHINFO` from `<sys/proc_info.h>`, missing from the `libc` crate.
const PROC_PIDFDVNODEPATHINFO: i32 = 2;

/// Blocking: paths of the files a process holds open; empty when it is gone or not ours.
pub(super) fn open_files(pid: u32) -> Vec<PathBuf> {
    let Ok(pid) = i32::try_from(pid) else {
        return Vec::new();
    };
    let mut descriptors = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        OPEN_FILES_LIMIT
    ];
    let capacity = std::mem::size_of_val(descriptors.as_slice());
    // The kernel fills at most `capacity` bytes and returns how many it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            descriptors.as_mut_ptr().cast(),
            capacity as i32,
        )
    };
    let Ok(written) = usize::try_from(written) else {
        return Vec::new();
    };
    descriptors.truncate(written / std::mem::size_of::<libc::proc_fdinfo>());
    descriptors
        .iter()
        .filter(|descriptor| descriptor.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
        .filter_map(|descriptor| descriptor_path(pid, descriptor.proc_fd))
        .collect()
}

/// `struct vnode_fdinfowithpath` from `<sys/proc_info.h>`. Only the path is read, so the
/// leading `struct proc_fileinfo` (two `u32`, an `off_t`, two 32-bit fields) is opaque.
#[repr(C)]
struct VnodeFdInfoWithPath {
    _file: [u64; 3],
    vnode: libc::vnode_info_path,
}

fn descriptor_path(pid: i32, descriptor: i32) -> Option<PathBuf> {
    let mut info = MaybeUninit::<VnodeFdInfoWithPath>::uninit();
    let size = std::mem::size_of::<VnodeFdInfoWithPath>();
    // The kernel initializes this fixed-size structure; inspect it only after a complete read.
    let read = unsafe {
        libc::proc_pidfdinfo(
            pid,
            descriptor,
            PROC_PIDFDVNODEPATHINFO,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info
        .vnode
        .vip_path
        .into_iter()
        .flatten()
        .take_while(|byte| *byte != 0)
        .map(|byte| byte as u8)
        .collect();
    let path = PathBuf::from(OsString::from_vec(bytes));
    path.is_absolute().then_some(path)
}

/// Blocking: when a process started; `None` when it is gone or not ours.
pub(super) fn start_time(pid: u32) -> Option<SystemTime> {
    let pid = i32::try_from(pid).ok()?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // The kernel initializes this fixed-size structure; inspect it only after a complete read.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    UNIX_EPOCH.checked_add(
        Duration::from_secs(info.pbi_start_tvsec) + Duration::from_micros(info.pbi_start_tvusec),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_files_lists_a_file_this_process_holds() {
        let path = std::env::temp_dir().join(format!("hive-open-files-{}", std::process::id()));
        let held = std::fs::File::create(&path).unwrap();
        let canonical = path.canonicalize().unwrap();
        assert!(open_files(std::process::id()).contains(&canonical));
        assert!(open_files(i32::MAX as u32).is_empty());
        drop(held);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn start_time_of_this_process_is_in_the_past() {
        let started = start_time(std::process::id()).unwrap();
        assert!(started <= SystemTime::now());
        assert!(SystemTime::now().duration_since(started).unwrap() < Duration::from_secs(3600));
        assert!(start_time(i32::MAX as u32).is_none());
    }
}
