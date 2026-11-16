import mmap
import tempfile


def current():
    data = b"\xff\x01"
    assert int.from_bytes(memoryview(data), "big", signed=True) == -255
    with tempfile.TemporaryFile() as backing:
        backing.write(data)
        backing.flush()
        with mmap.mmap(backing.fileno(), 0) as mapping, memoryview(mapping) as view:
            for source in (mapping, view):
                try:
                    int.from_bytes(source, "little")
                except NotImplementedError:
                    pass
                else:
                    raise AssertionError("mapped integer conversion lacks fallible limb allocation")
        try:
            int.from_bytes(mapping)
        except ValueError:
            pass
        else:
            raise AssertionError("accepted a closed mmap")
