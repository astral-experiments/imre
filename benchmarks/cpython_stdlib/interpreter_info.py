import platform
import site
import struct

print(platform.system())
print(platform.machine())
print(site.getsitepackages())
print(struct.unpack(">Q", bytes([0, 0, 0, 0, 0, 0, 0, 7])))
