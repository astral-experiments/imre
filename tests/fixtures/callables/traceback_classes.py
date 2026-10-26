import types
from types import FrameType
from types import TracebackType as TB

print(TB is types.TracebackType)
print(FrameType is types.FrameType)
print(TB is FrameType)
print(type(TB) is type)
print(TB.__name__)
print(TB.__module__)
print(FrameType.__name__)
print(FrameType.__module__)
print(repr(TB))
print(repr(FrameType))
