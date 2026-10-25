from pathlib import Path

bad = Path("bad\0component")
for op in range(8):
    try:
        if op == 0:
            bad.resolve()
        elif op == 1:
            Path("absent-before-nul/bad\0component").resolve(strict=True)
        elif op == 2:
            bad.mkdir(parents=True, exist_ok=True)
        elif op == 3:
            bad.unlink(missing_ok=True)
        elif op == 4:
            bad.rmdir()
        elif op == 5:
            bad.replace("destination")
        elif op == 6:
            Path("absent-source").replace(bad)
        else:
            bad.replace("bad\0destination")
    except ValueError as error:
        print(str(error))
    except OSError as error:
        print(repr(error.errno))
    else:
        raise AssertionError("expected an invalid path")
