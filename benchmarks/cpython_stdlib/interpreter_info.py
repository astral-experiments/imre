import platform
import site
import struct
import sys

assert platform.python_version() == sys.version.split()[0]
print(platform.system())
print(platform.machine())
print(site.getsitepackages())
print(struct.unpack(">Q", bytes([0, 0, 0, 0, 0, 0, 0, 7])))
