"""Keep formatters, real native records and their field mutations alive across calls."""
import logging
import time


def optional_text() -> str | None:
    if input() == "0":
        return None
    return input()


def run() -> None:
    formatters: list[logging.Formatter] = []
    records: list[logging.LogRecord] = []
    while True:
        command = input()
        if command == "stop":
            return
        try:
            if command == "formatter":
                fmt = optional_text()
                date = optional_text()
                style = input()
                validate = input() == "1"
                size = int(input())
                if size < 0:
                    f = logging.Formatter(fmt, date, style, validate)
                else:
                    defaults: dict[str, str] = {}
                    for i in range(size):
                        key = input()
                        defaults[key] = input()
                    f = logging.Formatter(fmt, date, style, validate, defaults=defaults)
                formatters.append(f)
                print(len(formatters) - 1)
            elif command == "record":
                shape = input()
                if shape == "message":
                    r = logging.makeLogRecord({"msg": input()})
                elif shape == "common":
                    name = input()
                    level = int(input())
                    path = input()
                    line = int(input())
                    msg = input()
                    n = int(input())
                    arg = input()
                    if input() == "1":
                        extra = int(input())
                        r = logging.makeLogRecord({"name": name, "level": level, "pathname": path,
                                                  "lineno": line, "exc_info": None, "func": None,
                                                  "msg": msg, "args": (n, arg), "custom": extra})
                    else:
                        r = logging.makeLogRecord({"name": name, "level": level, "pathname": path,
                                                  "lineno": line, "exc_info": None, "func": None,
                                                  "msg": msg, "args": (n, arg)})
                else:
                    raise ValueError(shape)
                records.append(r)
                print(len(records) - 1)
            elif command == "format":
                f = formatters[int(input())]
                r = records[int(input())]
                print(repr(f.format(r)))
            elif command == "formatTime":
                f = formatters[int(input())]
                r = records[int(input())]
                date = optional_text()
                print(repr(f.formatTime(r, date)))
            elif command == "_fmt":
                print(repr(formatters[int(input())]._fmt))
            elif command == "usesTime":
                print(formatters[int(input())].usesTime())
            elif command == "converter":
                f = formatters[int(input())]
                if input() == "gmtime":
                    f.converter = time.gmtime
                else:
                    f.converter = time.localtime
                print("None")
            elif command == "created":
                r = records[int(input())]
                r.created = float(input())
                print("None")
            elif command == "msecs-int":
                r = records[int(input())]
                r.msecs = int(input())
                print("None")
            elif command == "msecs-float":
                r = records[int(input())]
                r.msecs = float(input())
                print("None")
            elif command == "asctime":
                print(repr(records[int(input())].asctime))
            else:
                raise ValueError(command)
        except ValueError as error:
            print("!ValueError " + repr(str(error)))
        except TypeError as error:
            print("!TypeError " + repr(str(error)))


run()
