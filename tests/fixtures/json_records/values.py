from relar.json import decode


class Item:
    name: str
    metadata_bytes: int
    fields_bytes: int

    def __init__(self, name: str, metadata_bytes: int = 0, fields_bytes: int = 0) -> None:
        self.fields_bytes = fields_bytes
        self.name = name
        self.metadata_bytes = metadata_bytes


class Report:
    total: int
    structs: list[Item]
    queries: list[Item]

    def __init__(self, total: int, structs: list[Item] | None = None, queries: list[Item] | None = None) -> None:
        self.total = total
        self.structs = [] if structs is None else structs
        self.queries = [] if queries is None else queries


class Scalars:
    integer: int
    text: str
    flag: bool
    real: float
    nothing: None
    optional_int: int | None
    optional_bool: bool | None
    optional_float: float | None
    numbers: list[int]
    matrix: list[list[int]]
    items: list[Item | None]

    def __init__(self, integer: int, text: str, flag: bool, real: float, nothing: None, optional_int: int | None, optional_bool: bool | None, optional_float: float | None, numbers: list[int], matrix: list[list[int]], items: list[Item | None]) -> None:
        self.integer = integer
        self.text = text
        self.flag = flag
        self.real = real
        self.nothing = nothing
        self.optional_int = optional_int
        self.optional_bool = optional_bool
        self.optional_float = optional_float
        self.numbers = numbers
        self.matrix = matrix
        self.items = items

class Defaults:
    big: int
    text: str
    flag: bool
    real: float
    optional: str | None

    def __init__(self, big: int = -1267650600228229401496703205376, text: str = "default λ", flag: bool = True, real: float = -0.0, optional: str | None = "present") -> None:
        self.big = big
        self.text = text
        self.flag = flag
        self.real = real
        self.optional = optional


class Nullable:
    integer: int | None
    flag: bool | None
    real: float | None
    text: str | None
    items: list[Item] | None

    def __init__(self, integer: int | None = None, flag: bool | None = None, real: float | None = None, text: str | None = None, items: list[Item] | None = None) -> None:
        self.integer = integer
        self.flag = flag
        self.real = real
        self.text = text
        self.items = items


class Empty:
    def __init__(self) -> None:
        pass


class ListScalars:
    reals: list[float]
    flags: list[bool]
    names: list[str]
    nothing: list[None]
    optional_ints: list[int | None]
    optional_flags: list[bool | None]
    optional_reals: list[float | None]
    optional_names: list[str | None]

    def __init__(self, reals: list[float], flags: list[bool], names: list[str], nothing: list[None], optional_ints: list[int | None], optional_flags: list[bool | None], optional_reals: list[float | None], optional_names: list[str | None]) -> None:
        self.reals = reals
        self.flags = flags
        self.names = names
        self.nothing = nothing
        self.optional_ints = optional_ints
        self.optional_flags = optional_flags
        self.optional_reals = optional_reals
        self.optional_names = optional_names


def load(text: str):
    return decode(text, Report)


def source() -> str:
    print("source once")
    return '{"total":1267650600228229401496703205376,"structs":[{"name":"A λ","metadata_bytes":-4611686018427387905,"count":99},{"name":"\\ud83d\\ude00","fields_bytes":18446744073709551616}],"ignored":{"nested":[false,null,{"x":1}]}}'


report = load(source())
print(report.total)
for item in report.structs:
    print(f'{item.name} {item.metadata_bytes} {item.fields_bytes}')
print(len(report.queries))
first = load('{"total":1}')
second = load('{"total":2}')
first.structs.append(Item("only first"))
first.queries.append(Item("separate field"))
print(f'{len(first.structs)} {len(first.queries)} {len(second.structs)} {len(second.queries)}')

scalars = decode('{"integer":-1267650600228229401496703205376,"text":"NUL\\u0000é","flag":false,"real":-0.0,"nothing":null,"optional_int":4611686018427387904,"optional_bool":false,"optional_float":-0.0,"numbers":[0,-4611686018427387904,4611686018427387903,4611686018427387904],"matrix":[[],[1,2]],"items":[null,{"name":"nested"}]}', Scalars)
print(f'{scalars.integer} {scalars.text} {scalars.flag} {scalars.real} {scalars.nothing is None}')
maybe_int = scalars.optional_int
maybe_bool = scalars.optional_bool
maybe_float = scalars.optional_float
if maybe_int is not None:
    print(maybe_int)
if maybe_bool is not None:
    print(maybe_bool)
if maybe_float is not None:
    print(maybe_float)
print(f'{sum(scalars.numbers)} {len(scalars.matrix[0])} {sum(scalars.matrix[1])}')
for optional_item in scalars.items:
    if optional_item is None:
        print("null item")
    else:
        print(optional_item.name)


defaults = decode("{}", Defaults)
print(f'{defaults.big} {defaults.text} {defaults.flag} {defaults.real}')
optional_default = defaults.optional
if optional_default is not None:
    print(optional_default)
explicit_none = decode('{"optional":null}', Defaults)
print(explicit_none.optional is None)


nullable = decode("{}", Nullable)
print(f'{nullable.integer is None} {nullable.flag is None} {nullable.real is None} {nullable.text is None} {nullable.items is None}')


print(bool(decode('{"ignored":true}', Empty)))


lists = decode('{"reals":[-0.0,2,3.5],"flags":[false,true],"names":["","é"],"nothing":[null,null],"optional_ints":[null,0,-1267650600228229401496703205376],"optional_flags":[null,false,true],"optional_reals":[null,-0.0,3.5],"optional_names":[null,"","long string requiring independently owned heap storage beyond compact capacity"]}', ListScalars)
for real in lists.reals:
    print(real)
for flag in lists.flags:
    print(flag)
for name in lists.names:
    print(name)
print(all(value is None for value in lists.nothing))
for integer in lists.optional_ints:
    if integer is None:
        print("null int")
    else:
        print(integer)
for nullable_flag in lists.optional_flags:
    if nullable_flag is None:
        print("null bool")
    else:
        print(nullable_flag)
for nullable_real in lists.optional_reals:
    if nullable_real is None:
        print("null float")
    else:
        print(nullable_real)
for nullable_name in lists.optional_names:
    if nullable_name is None:
        print("null str")
    else:
        print(nullable_name)
