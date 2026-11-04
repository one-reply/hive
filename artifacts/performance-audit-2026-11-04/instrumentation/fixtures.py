from pathlib import Path
import subprocess,json
root=Path('/tmp/hive-perf-audit')
for n in [0,100,1000]:
 repo=root/f'repo-{n}'; repo.mkdir(exist_ok=True)
 def git(*args): subprocess.run(['git','-C',str(repo),*args],check=True,stdout=subprocess.DEVNULL)
 git('init','-b','main'); git('config','user.name','Audit'); git('config','user.email','audit@example.invalid'); git('config','commit.gpgsign','false')
 (repo/'tracked.txt').write_text('baseline\n'); git('add','.'); git('commit','-m','baseline')
 for i in range(n): (repo/f'file-{i:04}.txt').write_text('alpha beta\nsecond line\n'*50)
state=root/'state'; state.mkdir(exist_ok=True)
(state/'state.json').write_text(json.dumps({'projects':[{'path':str(root/f'repo-{n}')} for n in [0,100,1000]]}))
print('Created isolated 0/100/1000-file fixtures')
