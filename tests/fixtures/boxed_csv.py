import csv
from io import StringIO


def current():
    class Pipes(csv.excel):
        delimiter = "|"

    csv.register_dialect("pipes", Pipes)
    try:
        Pipes.delimiter = ";"
        fmt = csv.get_dialect(name="pipes")
        print(fmt.delimiter, fmt.doublequote, fmt.quoting)
        lines = iter(['a|"b\n', 'c"|d', 'x||y'])
        rows = csv.reader(lines, fmt)
        print(next(rows), rows.line_num, list(rows), rows.line_num)
    finally:
        csv.unregister_dialect(name="pipes")

    print(list(csv.reader(['"2",3,,'], quoting=csv.QUOTE_STRINGS)))
    print(list(csv.reader(['"a"'], quotechar=None)))
    limit = csv.field_size_limit(1)
    try:
        print(list(csv.reader(["é"])))
        try:
            next(csv.reader(["éé"]))
        except csv.Error as err:
            print(str(err))
    finally:
        csv.field_size_limit(limit)

    stream = StringIO()
    writer = csv.DictWriter(stream, ["name", "amount"], dialect="unix")
    print(writer.writeheader())
    writer.writerows([{"amount": 2, "name": 'a,"b"'}, {"name": "new\nline"}])
    print(repr(stream.getvalue()))
    print(list(csv.DictReader(["key,value", "x,1", "", "y,2,more"], restkey="extra")))

    class Syntax(str):
        def __len__(self):
            raise AssertionError("subclass length")

        def __contains__(self, other):
            raise AssertionError("subclass membership")

        def __radd__(self, other):
            raise AssertionError("subclass addition")

    stream = StringIO()
    ending = Syntax("|")
    writer = csv.writer(stream, delimiter=Syntax(";"), lineterminator=ending)
    print(type(writer.dialect.delimiter) is str, writer.dialect.lineterminator is ending)
    print(writer.writerow(["a;", "b|"]), stream.getvalue())
