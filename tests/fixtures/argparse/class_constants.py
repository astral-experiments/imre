import argparse


def parse():
    parser = argparse.ArgumentParser(prog="classes")
    parser.add_argument("--as-class", action="store_const", default=tuple, const=object,
                        help="%(default)s or %(const)s")
    parser.add_argument("--optional", action="store_const", const=int)
    parser.add_argument("--text", default="<class 'object'>")
    return parser.parse_args()


args = parse()
print(repr(args))
print(args.as_class is object)
print(args.as_class.__name__)
if args.optional is not None:
    print(args.optional.__name__)
