from pathlib import Path
import subprocess
import sys

# execve's filename selects the program, while argv[0] belongs to the caller.
original = ("unusual argv zero", "-c", 'printf "%s" "$0"')
p = subprocess.Popen(original, executable=b"/bin/sh", stdout=subprocess.PIPE)
print(repr(p.communicate()[0]))
print(repr(p.args))

with subprocess.Popen(
    ['printf "%s:%s:%s" "$FRUIT" "$0" "$1"', "name", "one arg"],
    stdout=subprocess.PIPE, shell=1, env={"FRUIT": "apple"}
) as shell:
    print(repr(shell.stdout.read()))
print(subprocess.call("exit 29", shell=True))
print(subprocess.call("exit 6", executable=Path("/bin/sh"), shell=True, cwd=b"."))
# Replacing the shell changes the executable before applying its -c flag.
print(subprocess.call("import sys; sys.exit(35)", executable=sys.argv[1], shell=True))
try:
    subprocess.Popen(Path("/bin/true"), shell=True)
except TypeError as error:
    print(str(error))
