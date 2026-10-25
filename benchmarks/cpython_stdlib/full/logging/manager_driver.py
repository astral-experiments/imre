# This program is appended to the verified native LoggerTest fixture.
import sys


def serve(case: NativeCase) -> None:
    logger = case.logger
    manager = logger.manager
    while True:
        op = input()
        if op == "stop":
            return
        if op == "manager":
            print(logger.manager is manager)
        elif op == "limit":
            print(manager.disable)
        elif op == "set-limit":
            manager.disable = int(input())
            print("ok")
        elif op == "disabled":
            print(logger.disabled)
        elif op == "set-disabled":
            logger.disabled = input() == "True"
            print("ok")
        elif op == "enabled":
            print(logger.isEnabledFor(int(input())))
        else:
            raise ValueError("unknown logger operation")
        sys.stdout.flush()


serve(NativeCase())
