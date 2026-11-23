use super::*;
use crate::host::{Host, as_login_shell};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Login shells to run real lines through, where this machine has them: macOS ships all but
/// fish. fish is also checked against its own quoting rules in `fish_reads_every_line_as_sh_does`.
const SHELLS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "/bin/dash",
    "/bin/ksh",
    "/bin/zsh",
    "/bin/csh",
    "/bin/tcsh",
    "/usr/bin/fish",
    "/usr/local/bin/fish",
    "/opt/homebrew/bin/fish",
];

pub(in crate::host) fn installed_shells() -> Vec<PathBuf> {
    SHELLS
        .iter()
        .map(PathBuf::from)
        .filter(|shell| shell.exists())
        .collect()
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "hive-login-shell-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("home")).unwrap();
        fs::create_dir_all(path.join("bin")).unwrap();
        Self(path)
    }
    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
    fn bin(&self) -> PathBuf {
        self.0.join("bin")
    }
    fn path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.bin().display())
    }
    fn executable(&self, name: &str, script: &str) {
        let path = self.bin().join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn remote() -> Host {
    Host::Ssh {
        target: "home".into(),
    }
}

fn split_nul(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut words: Vec<_> = bytes.split(|byte| *byte == 0).map(<[u8]>::to_vec).collect();
    assert_eq!(words.pop(), Some(vec![]), "every word ends with NUL");
    words
}

#[test]
fn every_login_shell_runs_the_same_argv_in_the_same_directory() {
    let scratch = Scratch::new("argv");
    let directory = scratch
        .0
        .join("it's \\'quoted\\' \\\\back\\\\  and $HOME `id` !1");
    fs::create_dir(&directory).unwrap();
    scratch.executable(
        "hive-argv",
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > argv\npwd -P > pwd\n",
    );
    let plain = [
        "a'b",
        "c\\'d",
        "e\\\\f",
        "g h",
        "i!j",
        "!!",
        "k\rl",
        "$(touch pwned)",
        "`touch pwned`",
        "*",
        "~",
        "%self",
        "{x,y}",
        "",
    ];
    // A newline ends a csh command even inside quotes; the rest must stay data.
    let multiline = ["one\ntouch pwned\n", "\n", "tail\\\n"];
    let physical = fs::canonicalize(&directory).unwrap();
    let shells = installed_shells();
    for args in [&plain[..], &multiline[..]] {
        let command = remote().command("hive-argv", args, Some(&directory));
        for shell in &shells {
            let output = as_login_shell(&command, shell, &scratch.home())
                .env("PATH", scratch.path())
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{}: {stderr}", shell.display());
            let argv = fs::read(directory.join("argv")).unwrap();
            let expected: Vec<Vec<u8>> = args.iter().map(|arg| arg.as_bytes().to_vec()).collect();
            assert_eq!(split_nul(&argv), expected, "{}", shell.display());
            assert_eq!(
                fs::read_to_string(directory.join("pwd")).unwrap(),
                format!("{}\n", physical.display())
            );
            fs::remove_file(directory.join("argv")).unwrap();
            assert!(!directory.join("pwned").exists(), "{}", shell.display());
            assert!(
                !scratch.home().join("pwned").exists(),
                "{}",
                shell.display()
            );
        }
    }
    assert!(shells.iter().any(|shell| shell.ends_with("sh")));
}

/// A stand-in `sh` that records the arguments the login shell handed it.
const RECORD_SH: &str = "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$HIVE_ARGV\"\n";

/// The script every installed login shell passes to `sh -c` for `line`.
fn scripts_given_to_sh(line: &OsStr) -> Vec<(PathBuf, Vec<u8>)> {
    let scratch = Scratch::new("record");
    scratch.executable("sh", RECORD_SH);
    let record = scratch.0.join("argv");
    let mut command = Command::new("/usr/bin/ssh");
    command.arg(line);
    installed_shells()
        .into_iter()
        .map(|shell| {
            let output = as_login_shell(&command, &shell, &scratch.home())
                .env("PATH", scratch.path())
                .env("HIVE_ARGV", &record)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{}: {stderr}", shell.display());
            let mut argv = split_nul(&fs::read(&record).unwrap());
            fs::remove_file(&record).unwrap();
            assert_eq!(argv.len(), 2, "{}", shell.display());
            assert_eq!(argv[0], b"-c", "{}", shell.display());
            (shell, argv.remove(1))
        })
        .collect()
}

#[test]
fn non_utf8_bytes_reach_sh_unchanged_from_every_login_shell() {
    // APFS refuses such names, so the directory only has to reach `sh`, not exist.
    let directory = PathBuf::from(OsString::from_vec(b"/gone/\xff'\\!x \xc3".to_vec()));
    let command = remote().command("pwd", &[], Some(&directory));
    for (shell, script) in scripts_given_to_sh(command.get_args().last().unwrap()) {
        assert_eq!(
            script,
            b"cd '/gone/\xff'\\''\\!x \xc3' && pwd",
            "{}",
            shell.display()
        );
    }
}

#[test]
fn a_script_with_newlines_is_rebuilt_inside_sh() {
    let posix = OsStr::new("printf '%s\\n' 'a\nb' \\\\ \"\\c\" '\\0101'\n");
    let line = exec_sh(posix);
    assert!(!line.as_bytes().contains(&b'\n'));
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&line)
        .output()
        .unwrap();
    let expected = Command::new("/bin/sh")
        .arg("-c")
        .arg(posix)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, expected.stdout);
    assert_eq!(output.stdout, b"a\nb\n\\\n\\c\n\\0101\n");
}

/// Splits `line` into words as fish's tokenizer and `unescape_string` do (fish-shell
/// `crates/common/src/lib.rs`, `unescape_string_internal` and `unescape_one`, read at
/// f0d874ad9): inside single quotes only `\\` and `\'` are escapes; outside them a backslash
/// keeps the next character, except for letters and digits that fish turns into control
/// characters or numeric escapes. Anything fish would expand unquoted fails the test.
fn fish_words(line: &[u8]) -> Vec<Vec<u8>> {
    let mut words = Vec::new();
    let mut word: Option<Vec<u8>> = None;
    let mut quoted = false;
    let mut bytes = line.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        if quoted {
            match byte {
                b'\'' => quoted = false,
                b'\\' if matches!(bytes.peek(), Some(b'\\' | b'\'')) => {
                    word.get_or_insert_default().push(bytes.next().unwrap())
                }
                _ => word.get_or_insert_default().push(byte),
            }
            continue;
        }
        match byte {
            b' ' => words.extend(word.take()),
            b'\'' => {
                quoted = true;
                word.get_or_insert_default();
            }
            b'\\' => {
                let next = bytes
                    .next()
                    .expect("a trailing backslash continues the line");
                assert!(
                    !next.is_ascii_alphanumeric() && next != b'\n',
                    "fish reads \\{} as an escape sequence",
                    next as char
                );
                word.get_or_insert_default().push(next);
            }
            b'$' | b'(' | b')' | b';' | b'&' | b'|' | b'<' | b'>' | b'*' | b'?' | b'~' | b'{'
            | b'}' | b'[' | b']' | b'#' | b'"' | b'%' | b'\n' | b'\t' => {
                panic!("fish treats an unquoted {:?} specially", byte as char)
            }
            _ => word.get_or_insert_default().push(byte),
        }
    }
    assert!(!quoted, "unterminated quote");
    words.extend(word);
    words
}

#[test]
fn fish_reads_every_line_as_sh_does() {
    let directory = PathBuf::from(OsString::from_vec(
        b"/srv/a'b \\' \\\\ !x $(id) `id` \xff".to_vec(),
    ));
    let commands = [
        remote().command("git", &["log", "--format=%s it's \\ !"], Some(&directory)),
        remote().command("git", &["commit", "-m", "subject\n\nbody's \\ !"], None),
        remote().login_command("exec rust-analyzer", Some(&directory)),
        remote()
            .terminal_command(&directory, Some(("codex", &["--model", "it's \\ !"])))
            .unwrap(),
    ];
    for command in &commands {
        let line = command.get_args().last().unwrap();
        let (_, by_sh) = scripts_given_to_sh(line)
            .into_iter()
            .find(|(shell, _)| shell.ends_with("sh"))
            .expect("/bin/sh is installed");
        let words = fish_words(line.as_bytes());
        assert_eq!(
            words,
            [b"exec".to_vec(), b"sh".to_vec(), b"-c".to_vec(), by_sh],
            "{line:?}"
        );
    }
}

#[test]
fn the_login_shell_argument_reads_the_same_in_fish() {
    // `in_login_environment` hands the login shell a second line; fish must read it too.
    let posix = in_login_environment(OsStr::new("printf '%s' 'it'\\''s \\ !'"));
    let posix = posix.as_bytes();
    let start = posix
        .windows(b"\"$login\" ".len())
        .position(|window| window == b"\"$login\" ")
        .unwrap()
        + b"\"$login\" ".len();
    let mut print = b"printf %s ".to_vec();
    print.extend_from_slice(&posix[start..]);
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(OsStr::from_bytes(&print))
        .output()
        .unwrap();
    assert!(output.status.success());
    let argument = output.stdout;
    let words = fish_words(&argument);
    assert_eq!(
        words[..3],
        [b"exec".to_vec(), b"sh".to_vec(), b"-c".to_vec()]
    );
    let mut script = INTERACTIVE_PATH.as_bytes().to_vec();
    script.extend_from_slice(b"printf '%s' 'it'\\''s \\ !'");
    assert_eq!(words[3], script);
}
