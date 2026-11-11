import argparse


def read(repository: str) -> str:
    parser = argparse.ArgumentParser(prog="repository")
    parser.add_argument("--repository", default=repository, help=f"Use {repository}")
    repository = "after registration"
    return parser.parse_args().repository


print(read("initial value"))
