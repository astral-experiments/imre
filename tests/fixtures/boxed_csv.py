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

    class Product:
        def __str__(self):
            return Syntax('12" box')

    stream = StringIO()
    assert csv.writer(stream).writerow([Product()]) == 12
    assert stream.getvalue() == '"12"" box"\r\n'

    escaped_byte = b"\xff".decode("utf-8", "surrogateescape")
    name = "report_" + escaped_byte + ".csv"
    lines = [name + ",tail", '"bad"' + escaped_byte, "\\" + escaped_byte, '"\\' + escaped_byte + '"']
    rows = csv.reader(iter(lines), strict=True, escapechar="\\")
    first = next(rows)
    try:
        next(rows)
    except csv.Error:
        pass
    else:
        raise AssertionError("expected strict quote error")
    assert next(rows) == [escaped_byte]
    assert next(rows) == [escaped_byte]
    assert first == [name, "tail"]
