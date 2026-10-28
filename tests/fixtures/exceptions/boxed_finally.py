import sys
sys.path

class M:
    def __init__(self, log):
        self.log = log
    def __enter__(self):
        self.log.append("enter")
        return self
    def __exit__(self, t, e, tb):
        self.log.append("exit")

def operation(mode, log):
    try:
        with M(log):
            try:
                log.append("body")
                if mode in ("raise", "replace", "reraise"):
                    raise ValueError("body error")
                if mode == "return":
                    return ["from body"]
            finally:
                log.append("inner")
                if mode == "replace":
                    raise RuntimeError("finish error")
                if mode == "reraise":
                    raise
    except ValueError as e:
        log.append(e.args[0])
    else:
        log.append("else")
    finally:
        log.append("outer")
        if mode == "override":
            return ["from finally"]
    return ["fallthrough"]

for mode in ("ok", "return", "raise", "replace", "reraise", "override"):
    seen = []
    try:
        value = operation(mode, seen)
        print(mode, value, seen)
    except RuntimeError as err:
        print(mode, err.args[0], seen)

def loop():
    log = []
    for i in range(4):
        try:
            if i % 2 == 0: continue
            if i == 3: break
            log.append(11)
        finally:
            log.append(i)
    return log
print(loop())

def replacement():
    for i in range(3):
        try:
            return 9
        finally:
            if i == 0:
                continue
            break
    return 7
print(replacement())

def nested_loop(again):
    seen = []
    for outer in range(2):
        try:
            for inner in range(2):
                seen.append((outer, inner))
                return seen
        finally:
            if outer == 0:
                if again:
                    continue
                break
    return seen

print(nested_loop(True), nested_loop(False))

def leave_handler(mode, log):
    for _ in range(1):
        try:
            try:
                raise ValueError("inner")
            except ValueError as error:
                if mode == "break":
                    break
                if mode == "continue":
                    continue
                if mode == "raise":
                    raise KeyError("replacement")
                return
        finally:
            try:
                log.append(str(error))
            except NameError:
                log.append("unbound")
            raise

for mode in ("break", "continue", "raise", "return"):
    log = []
    try:
        leave_handler(mode, log)
    except Exception as exc:
        print(mode, log, type(exc).__name__)

class ReraisingExit(M):
    def __exit__(self, kind, error, traceback):
        self.error = error
        return self

    def __bool__(self):
        self.log.append(sys.exception() is self.error)
        raise

log = []
with ReraisingExit(log):
    pass
try:
    raise KeyError("caller")
except KeyError:
    try:
        with ReraisingExit(log):
            raise ValueError("body")
    except ValueError:
        pass
assert log == ["enter", "enter", True]

class ActiveExit(M):
    def __exit__(self, kind, error, tb):
        self.log.append(type(sys.exception()).__name__)

def exit_after_handler(early, log):
    with ActiveExit(log):
        try:
            raise ValueError("handled")
        except ValueError:
            if early:
                return

for early in (False, True):
    log = []
    exit_after_handler(early, log)
    try:
        raise KeyError("caller")
    except KeyError:
        exit_after_handler(early, log)
    print(log)

def buffered(path, saved):
    stream = open(path, "wb")
    stream.write(b"pending")
    if saved is not None:
        saved.append(stream)
    return stream

def discard_pending(path, saved, again, raised):
    for _ in range(1):
        try:
            if raised:
                raise ValueError([buffered(path, saved)])
            return buffered(path, saved)
        finally:
            if again:
                continue
            break
    with open(path, "rb") as reader:
        return reader.read()

for again in (False, True):
    for raised in (False, True):
        assert discard_pending("finally-return.bin", None, again, raised) == b"pending"
        saved = []
        assert discard_pending("finally-return.bin", saved, again, raised) == b""
        saved[0].close()
