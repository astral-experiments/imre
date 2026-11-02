import os
import subprocess
from os import getpid as current_pid
from sys import argv

parent = subprocess.check_output([argv[1], "-I", "-c", "import os; print(os.getppid())"], text=True)
print(os.getpid() == int(parent))
print(current_pid() == os.getpid())
