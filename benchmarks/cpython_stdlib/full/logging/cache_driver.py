"""Keep the same native loggers alive while the upstream test makes its assertions."""
import logging
import sys


def main() -> None:
    logging.basicConfig(level=logging.DEBUG, format="%(name)s -> %(levelname)s: %(message)s")
    root = logging.getLogger()
    loggers: dict[str, logging.Logger] = {root.name: root}
    print(str(root.level) + ":" + str(root.hasHandlers()))
    sys.stdout.flush()
    while True:
        op = input()
        if op == "stop":
            return
        if op == "disable":
            logging.disable()
            print("ok")
        elif op == "get":
            name = input()
            logger = logging.getLogger(name)
            loggers[name] = logger
            print(logger.name)
        else:
            logger = loggers[input()]
            if op == "set":
                logger.setLevel(int(input()))
                print("ok")
            elif op == "effective":
                print(logger.getEffectiveLevel())
            elif op == "enabled":
                print(logger.isEnabledFor(int(input())))
            elif op == "cache":
                print(logger._cache)
            else:
                raise ValueError("unknown logger cache operation")
        sys.stdout.flush()


main()
