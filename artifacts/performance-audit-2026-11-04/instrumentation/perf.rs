use std::sync::{Mutex, OnceLock};
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
