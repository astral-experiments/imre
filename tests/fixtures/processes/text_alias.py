import subprocess
import sys


def missing() -> list[str]:
    print("arguments evaluated")
    return ["missing-text-alias-executable"]


echo = [sys.argv[1], "-c", "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read()+b'\\rEND\\r\\n'); sys.stderr.buffer.write(b'one\\rtwo\\n')"]
with subprocess.Popen(echo, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                      universal_newlines=1, text=True, pass_fds=(), umask=0o077) as proc:
    print(repr(proc.communicate("λ\r\n")))
with subprocess.Popen(echo, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                      universal_newlines=0, text=False) as binary:
    print(repr(binary.communicate(b"\xff")))
with subprocess.Popen(echo, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                      universal_newlines=-1, text=None) as text:
    print(repr(text.communicate()))
with subprocess.Popen(echo, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                      universal_newlines=None, text=True) as explicit:
    print(repr(explicit.communicate()))

child = [sys.argv[1], "-c", "pass"]
print(subprocess.call(child, universal_newlines=True, timeout=30))
print(subprocess.check_call(child, universal_newlines=False))

try:
    subprocess.Popen(missing(), text=True, universal_newlines=0, stdout=subprocess.STDOUT)
except ValueError as error:
    print(str(error))
try:
    subprocess.Popen(missing(), text=True, universal_newlines=0)
except subprocess.SubprocessError as error:
    print(str(error))
try:
    subprocess.call(missing(), universal_newlines=1, text=False)
except subprocess.SubprocessError as error:
    print(str(error))
try:
    subprocess.check_call(missing(), text=True, universal_newlines=False)
except subprocess.SubprocessError as error:
    print(str(error))

try:
    subprocess.run(missing(), universal_newlines=True, text=False, stdout=subprocess.STDOUT)
except ValueError as error:
    print(str(error))
