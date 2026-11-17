import glob
import os
import tempfile
import types


def current():
    with tempfile.TemporaryDirectory() as root:
        os.mkdir(os.path.join(root, "left"))
        os.mkdir(os.path.join(root, "right"))
        for directory in (root, os.path.join(root, "left"), os.path.join(root, "right")):
            with open(os.path.join(directory, "file.txt"), "w") as file:
                file.write("x")

        def descriptors():
            return len(os.listdir("/proc/self/fd")) if os.path.isdir("/proc/self/fd") else 0

        def scan(fail=False):
            with os.scandir(root) as entries:
                yield next(entries).name
                if fail:
                    raise ValueError("scan failed")

        baseline = descriptors()
        live = [scan() for _ in range(12)]
        for iterator in live:
            assert type(iterator) is types.GeneratorType
            assert iter(iterator) is iterator
            next(iterator)
        if baseline:
            assert descriptors() == baseline + len(live)
        live[0].close()
        if baseline:
            assert descriptors() == baseline + len(live) - 1
        for iterator in live[1:]:
            assert list(iterator) == []
        if baseline:
            assert descriptors() == baseline

        iterator = scan(True)
        next(iterator)
        try:
            next(iterator)
        except ValueError as error:
            assert str(error) == "scan failed"
        else:
            raise AssertionError("scan did not fail")
        if baseline:
            assert descriptors() == baseline

        pattern = os.path.join(root, "**", "*.txt")
        iterator = glob.iglob(pattern, recursive=True)
        assert type(iterator) is types.GeneratorType
        next(iterator)
        iterator.close()
        assert len(list(glob.iglob(pattern, recursive=True))) == 3
        if baseline:
            assert descriptors() == baseline
    print("generator identity, iteration, exhaustion, close, error, and glob cleanup")
