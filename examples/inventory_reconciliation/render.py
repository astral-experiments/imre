from typing import TextIO

from .model import Fulfillment, Report


def line(output: TextIO, value: str) -> None:
    output.write(value + "\n")


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
    line(output, "STOCK_ROWS " + str(report.stock_rows))
    line(output, "STOCK_SKUS " + str(len(report.remaining)))
    line(output, "BATCHES " + str(report.batches))
    line(output, "REQUEST_ROWS " + str(report.request_rows))
    line(output, "POSITIVE_REQUESTS " + str(len(report.results)))
    line(output, "REQUESTED_SKUS " + str(len(report.requested_skus)))
    line(output, "TOTAL_REQUESTED " + str(requested_total))
    line(output, "TOTAL_FILLED " + str(filled_total))
    line(output, "TOTAL_SHORTAGE " + str(shortage_total))
    line(output, "FULFILLMENTS " + str(len(report.results)))
    for result in report.results:
        request = result.request
        line(
            output,
            "FULFILL "
            + str(request.batch)
            + " "
            + str(request.row)
            + " "
            + request.sku
            + " "
            + str(request.quantity)
            + " "
            + str(result.filled)
            + " "
            + str(result.shortage),
        )
    line(output, "SHORTAGES " + str(len(shortages)))
    for result in shortages:
        request = result.request
        line(
            output,
            "SHORTAGE "
            + str(request.batch)
            + " "
            + str(request.row)
            + " "
            + request.sku
            + " "
            + str(result.shortage),
        )
    line(output, "REQUESTED " + str(len(report.requested)))
    for sku, quantity in report.requested.items():
        line(output, "REQUEST " + sku + " " + str(quantity))
    line(output, "REMAINING " + str(len(report.remaining)))
    for sku, quantity in report.remaining.items():
        line(output, "STOCK " + sku + " " + str(quantity))
    line(output, "UNTOUCHED " + str(len(untouched)))
    for sku, quantity in untouched:
        line(output, "STOCK " + sku + " " + str(quantity))
    line(output, "END")
