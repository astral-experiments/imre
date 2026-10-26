from _string import formatter_field_name_split as split, formatter_parser as parse

def value(label: str) -> str:
    print(label)
    return label

def fail() -> str:
    raise ValueError("second argument")

try:
    parse(1)
except TypeError as error:
    print(str(error))

try:
    parse(None)
except TypeError as error:
    print(str(error))

try:
    parse()
except TypeError as error:
    print(str(error))

try:
    parse(value("a"), value("b"))
except TypeError as error:
    print(str(error))

try:
    parse(string=value("key"))
except TypeError as error:
    print(str(error))

try:
    parse(value("first"), fail())
except ValueError as error:
    print(str(error))

try:
    split(1)
except TypeError as error:
    print(str(error))

try:
    split(None)
except TypeError as error:
    print(str(error))

try:
    split()
except TypeError as error:
    print(str(error))

try:
    split(value("a"), value("b"))
except TypeError as error:
    print(str(error))

try:
    split(string=value("key"))
except TypeError as error:
    print(str(error))

try:
    split(value("first"), fail())
except ValueError as error:
    print(str(error))
