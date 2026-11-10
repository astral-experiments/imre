import argparse


class Target:
    pass


def run():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("report")
    result: object = parser.parse_args(["report"], namespace=Target)
    print(result is Target, Target.command)


run()
