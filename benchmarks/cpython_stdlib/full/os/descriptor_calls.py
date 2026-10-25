"""Entry point for upstream tests that inspect a native fd from the host process."""
import os
import sys


op = sys.argv[1]
if op == "open":
    print(os.open(sys.argv[2], int(sys.argv[3]), int(sys.argv[4])))
elif op == "close":
    os.close(int(sys.argv[2]))
elif op == "dup":
    print(os.dup(int(sys.argv[2])))
elif op == "dup2":
    print(os.dup2(int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))
elif op == "get_inheritable":
    print(os.get_inheritable(int(sys.argv[2])))
elif op == "set_inheritable":
    os.set_inheritable(int(sys.argv[2]), int(sys.argv[3]))
else:
    raise ValueError("unknown operation")
