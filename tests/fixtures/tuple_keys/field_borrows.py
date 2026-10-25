from dataclasses import dataclass


@dataclass
class Row:
    table: dict[tuple[int], str]
    rank: tuple[int]


def replace(row: Row, fail: bool) -> str:
    row.rank = (99,)
    row.table = {row.rank: "new"}
    if fail:
        raise ValueError("default failed")
    return "missing"


def replace_rank(row: Row) -> tuple[int]:
    row.rank = (0,)
    return (3,)


def exercise(row: Row, peer: Row) -> None:
    print(row.table.get(row.rank) is None)
    print(row.rank < peer.rank)
    # The eager default replaces both owners after the receiver and key loads.
    print(row.table.get(row.rank, replace(row, False)))
    try:
        print(row.table.get(row.rank, replace(row, True)))
    except ValueError as error:
        print(str(error))
        print(row.table.get(row.rank, "missing"))
    # The middle tuple must survive the call that overwrites its field.
    print(peer.rank < row.rank < replace_rank(row))


exercise(Row({(1,): "old"}, (1,)), Row({}, (2,)))
