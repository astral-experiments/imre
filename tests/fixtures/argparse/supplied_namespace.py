import argparse


class Target:
    label = "existing"


def run():
    parser = argparse.ArgumentParser(prog="namespace")
    parser.add_argument("--label", default="parser default")
    parser.add_argument("--count", type=int, default=11)
    target = Target()
    result = parser.parse_args(["--count", "7"], target)
    print(result is target, result.label, result.count)
    result, extras = parser.parse_known_args(["--extra"], namespace=Target)
    print(result is Target, Target.label, Target.count, extras)


run()
