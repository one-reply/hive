use hive_core::{git::status, host::Host};
use std::{path::Path, time::Instant};
fn main() {
 if std::env::args().any(|a| a == "stall") {
   status::read(&Host::Local, Path::new("/tmp/hive-perf-audit/repo-0")).unwrap();
   return;
 }
 for n in [0,100,1000] {
  let path=format!("/tmp/hive-perf-audit/repo-{n}");
  for run in 0..4 {
   let start=Instant::now();
   let base=status::read(&Host::Local,Path::new(&path)).unwrap();
   let base_ms=start.elapsed().as_secs_f64()*1000.;
   let start=Instant::now();
   let full=status::read_with_stats(&Host::Local,Path::new(&path)).unwrap();
   let full_ms=start.elapsed().as_secs_f64()*1000.;
   assert_eq!(base.files.len(),n); assert_eq!(full.files.len(),n);
   assert!(full.files.iter().all(|f| f.unstaged_stats.is_some_and(|s|s.added==100 && s.removed==0)));
   println!("{n},{run},{base_ms:.3},{full_ms:.3}");
  }
 }
}
