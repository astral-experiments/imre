import sys

from .load import load, read_orders, read_stock
from .reconcile import reconcile
from .render import show


def main() -> None:
    _, *arguments = sys.argv
    try:
        stock_path, orders_path, report_path = arguments
    except ValueError:
        raise ValueError("usage: inventory_reconciliation STOCK ORDERS REPORT")
    stocks = read_stock(load(stock_path))
    batches = read_orders(load(orders_path, label="orders"))
    report = reconcile(stocks, batches=batches)
    # Both inputs are closed before any output truncation, including aliases.
    try:
        with open(report_path, "w", encoding="utf-8") as output:
            show(report, output=output)
            output.flush()
    except OSError:
        raise OSError("report: cannot write")
    except ValueError:
        raise ValueError("report: invalid path")


main()
