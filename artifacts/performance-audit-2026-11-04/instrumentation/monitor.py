import subprocess,time,pathlib,sys
root=pathlib.Path('/tmp/hive-perf-audit')
rows=subprocess.check_output(['ps','-axo','pid,comm'],text=True).splitlines()
pids=[int(r.split()[0]) for r in rows if '/tmp/hive-perf-audit/HivePerfAudit.app/Contents/MacOS/hive' in r]
pid=int(sys.argv[1]) if len(sys.argv)>1 else pids[0]; (root/'pid').write_text(str(pid)); print(pid,flush=True)
with (root/'resources.csv').open('a') as out:
 for _ in range(1800):
  p=subprocess.run(['ps','-p',str(pid),'-o','%cpu=,rss=,time='],capture_output=True,text=True)
  if not p.stdout.strip(): break
  out.write(str(time.time())+','+','.join(p.stdout.split())+'\n');out.flush();time.sleep(1)
