from argparse import ArgumentParser


def word(value: str) -> str:
    print(value)
    return value


class Commands:
    def checked(self):
        parser = ArgumentParser(prog="method")
        parser.add_argument("words", nargs=2)
        parser.parse_args((word("first"), word("second")))

    def missing(self):
        parser = ArgumentParser(prog="method")
        parser.add_argument("word")
        parser.parse_args(())


commands = Commands()
commands.checked()
try:
    commands.missing()
except SystemExit as error:
    print(error.code)
