from pathlib import Path
p=Path('/tmp/hive-perf-audit/source/crates/hive/src')
(p/'perf.rs').write_text('''use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::io::Write;
static SAMPLES: OnceLock<Mutex<Vec<(u128, &'static str, u128)>>> = OnceLock::new();
pub fn record(name: &'static str, us: u128) {
 let mut data = SAMPLES.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap();
 if data.len() < 100000 { data.push((SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros(),name,us)); }
}
pub struct Timer(&'static str, Instant);
impl Timer { pub fn new(name: &'static str) -> Self { Self(name, Instant::now()) } }
impl Drop for Timer { fn drop(&mut self) { record(self.0,self.1.elapsed().as_micros()); } }
pub fn start(cx: &mut gpui_kit::App) {
 std::thread::spawn(|| {
  let mut out=std::fs::OpenOptions::new().create(true).append(true).open("/tmp/hive-perf-audit/timing.csv").unwrap();
  loop { std::thread::sleep(std::time::Duration::from_secs(1));
   let data=std::mem::take(&mut *SAMPLES.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap());
   for (ts,name,us) in data { writeln!(out,"{ts},{name},{us}").unwrap(); }
   out.flush().unwrap();
  }
 });
 cx.spawn(async |cx| { loop {
   let start=Instant::now();
   cx.background_executor().timer(std::time::Duration::from_millis(8)).await;
   record("heartbeat_lateness", start.elapsed().as_micros().saturating_sub(8000));
 }}).detach();
}
''')
f=p/'main.rs'; s=f.read_text().replace('mod app;','mod app;\nmod perf;').replace('gpui_kit::init(cx);','gpui_kit::init(cx);\n            perf::start(cx);'); f.write_text(s)
f=p/'terminal/element.rs'; s=f.read_text().replace('        let theme = cx.theme();\n        let font_size', '        let _perf = crate::perf::Timer::new("terminal_prepaint");\n        let theme = cx.theme();\n        let font_size').replace('        let term = terminal.term().lock();','        let wait = std::time::Instant::now();\n        let term = terminal.term().lock();\n        crate::perf::record("terminal_lock_wait", wait.elapsed().as_micros());\n        let hold = std::time::Instant::now();').replace('        drop(term);','        drop(term);\n        crate::perf::record("terminal_lock_hold", hold.elapsed().as_micros());'); f.write_text(s)
f=p/'ui/git_panel/files.rs'; s=f.read_text().replace('        let state = self.git.read(cx);','        let _perf = crate::perf::Timer::new("git_changes_render");\n        let state = self.git.read(cx);'); f.write_text(s)
f=p/'state/git.rs'; s=f.read_text().replace('let status = git::status::read_with_stats(&t.host, &t.path)?;','let timer = crate::perf::Timer::new("git_status_stats");\n                    let status = git::status::read_with_stats(&t.host, &t.path)?;\n                    drop(timer);'); f.write_text(s)
f=p/'state/git_diff.rs'; s=f.read_text().replace('let patch = diff::read(', 'let _perf = crate::perf::Timer::new("diff_read_parse");\n                    let patch = diff::read('); f.write_text(s)
# Clean shell with no user startup files or network commands.
f=Path('/tmp/hive-perf-audit/source/crates/hive/src/state/terminal_session.rs'); s=f.read_text().replace('cwd.as_deref(), None, TerminalSize','cwd.as_deref(), Some("/bin/sh"), TerminalSize'); f.write_text(s)
