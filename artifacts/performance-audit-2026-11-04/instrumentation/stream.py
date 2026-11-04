import sys,time
seconds=float(sys.argv[1]) if len(sys.argv)>1 else 30
end=time.monotonic()+seconds
line='\033[32mHive audit\033[0m alpha beta 0123456789 '*3+'界 e\u0301\n'
while time.monotonic()<end:
 sys.stdout.write(line*10);sys.stdout.flush();time.sleep(1/60)
print('AUDIT_STREAM_DONE')
