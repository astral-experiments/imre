import sys

from .load import load, read_orders, read_stock
from .reconcile import reconcile
from .render import show


def main() -> None:
    if len(sys.argv) != 4:
        raise ValueError("usage: inventory_reconciliation STOCK ORDERS REPORT")
    stocks = read_stock(load(sys.argv[1]))
    batches = read_orders(load(sys.argv[2], label="orders"))
    report = reconcile(stocks, batches=batches)
    # Both inputs are closed before any output truncation, including aliases.
    try:
        with open(sys.argv[3], "w", encoding="utf-8") as output:
            show(report, output=output)
            output.flush()
    except OSError:
        raise OSError("report: cannot write")
    except ValueError:
        raise ValueError("report: invalid path")


main()
