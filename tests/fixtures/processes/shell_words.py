import shlex
import subprocess
import sys

for text in ["é#x \t''", r"a\ b 'two words'c", r'"a\$b\\c\""', "open'", "escape\\", '"bad\\', "x\u00a0y", "a\\\nb"]:
    try:
        print(repr(shlex.split(text)))
    except ValueError as error:
        print(str(error))
try:
    subprocess.run([sys.argv[1], "-c", "import sys; sys.exit(9)"], stdout=sys.stderr, check=True)
except subprocess.CalledProcessError as error:
    print(error.returncode)
    print(error.stdout is None)
    print(error.stderr is None)
try:
    sys.exit()
except SystemExit as error:
    print(repr(str(error)))
try:
    sys.exit(257)
except SystemExit as error:
    print(str(error))
