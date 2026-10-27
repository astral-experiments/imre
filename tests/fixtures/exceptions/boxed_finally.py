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
