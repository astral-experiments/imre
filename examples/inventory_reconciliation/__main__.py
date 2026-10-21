import sys

from .load import load, read_orders, read_stock
from .reconcile import reconcile
from .render import show


def main() -> None:
    arguments = sys.argv[1:]
    if len(arguments) != 3:
        raise ValueError("usage: inventory_reconciliation STOCK ORDERS REPORT")
    stocks = read_stock(load(arguments[0]))
    batches = read_orders(load(arguments[1], label="orders"))
    report = reconcile(stocks, batches=batches)
    # Both inputs are closed before any output truncation, including aliases.
    try:
        with open(arguments[2], "w", encoding="utf-8") as output:
            show(report, output=output)
            output.flush()
    except OSError:
        raise OSError("report: cannot write")
    except ValueError:
        raise ValueError("report: invalid path")


main()
