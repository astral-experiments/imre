from typing import TextIO

from .model import Fulfillment, Report


def line(output: TextIO, value: str) -> None:
    output.write(f"{value}\n")


def show(report: Report, output: TextIO) -> None:
    shortages: list[Fulfillment] = [
        result for result in report.results if result.shortage > 0
    ]
    untouched: list[tuple[str, int]] = [
        (sku, quantity)
        for sku, quantity in report.remaining.items()
        if sku not in report.requested_skus
    ]
    requested_total = 0
    filled_total = 0
    shortage_total = 0
    for result in report.results:
        requested_total += result.request.quantity
        filled_total += result.filled
        shortage_total += result.shortage
    line(output, "INVENTORY")
    line(output, f"STOCK_ROWS {report.stock_rows}")
    line(output, f"STOCK_SKUS {len(report.remaining)}")
    line(output, f"BATCHES {report.batches}")
    line(output, f"REQUEST_ROWS {report.request_rows}")
    line(output, f"POSITIVE_REQUESTS {len(report.results)}")
    line(output, f"REQUESTED_SKUS {len(report.requested_skus)}")
    line(output, f"TOTAL_REQUESTED {requested_total}")
    line(output, f"TOTAL_FILLED {filled_total}")
    line(output, f"TOTAL_SHORTAGE {shortage_total}")
    line(output, f"FULFILLMENTS {len(report.results)}")
    for result in report.results:
        request = result.request
        line(
            output,
            f"FULFILL {request.batch} {request.row} {request.sku} "
            f"{request.quantity} {result.filled} {result.shortage}",
        )
    line(output, f"SHORTAGES {len(shortages)}")
    for result in shortages:
        request = result.request
        line(
            output,
            f"SHORTAGE {request.batch} {request.row} {request.sku} {result.shortage}",
        )
    line(output, f"REQUESTED {len(report.requested)}")
    for sku, quantity in report.requested.items():
        line(output, f"REQUEST {sku} {quantity}")
    line(output, f"REMAINING {len(report.remaining)}")
    for sku, quantity in report.remaining.items():
        line(output, f"STOCK {sku} {quantity}")
    line(output, f"UNTOUCHED {len(untouched)}")
    for sku, quantity in untouched:
        line(output, f"STOCK {sku} {quantity}")
    line(output, "END")
