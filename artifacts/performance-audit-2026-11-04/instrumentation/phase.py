import sys,time
with open('/tmp/hive-perf-audit/phases.csv','a') as f: f.write(f'{time.time()},{sys.argv[1]}\n')
