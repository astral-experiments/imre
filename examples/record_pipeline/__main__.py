from sys import argv
from typing import Callable

from .load import load, number
from .pipeline import select_predicate, select_transform, summarize


def decimal(value: int) -> str:
    text = str(value)
    if len(text) - (1 if value < 0 else 0) > 4300:
        raise ValueError(
            "Exceeds the limit (4300 digits) for integer string conversion; "
            "use sys.set_int_max_str_digits() to increase the limit"
        )
    return text


def main() -> None:
    try:
        if len(argv) != 7:
            raise ValueError(
                "usage: record_pipeline MODE INPUT REPORT LIMIT FEE REPEATS"
            )
        _, mode, input_path, output_path, limit_text, fee_text, repeat_text = argv
        limit = number(limit_text, "limit")
        fee = number(fee_text, "fee")
        repetitions = number(repeat_text, "repetitions")
        if limit < 0 or fee < 0:
            raise ValueError("limit and fee must be nonnegative")
        if repetitions < 1:
            raise ValueError("repetitions must be positive")
        accept = select_predicate(limit)
        transform = select_transform(mode, fee)
        positive: Callable[[int], int] = lambda amount: amount if amount > 0 else 0
        records = load(input_path)
        count = 0
        total = 0
        largest = 0
        checksum = 0
        for repetition in range(repetitions):
            count, total, largest = summarize(records, accept, transform, positive)
            checksum += count + total + largest
        count_text = decimal(count)
        total_text = decimal(total)
        largest_text = decimal(largest)
        checksum_text = decimal(checksum)
        report = (
            "PAYOUTS\n"
            f"COUNT {count_text}\nTOTAL {total_text}\nLARGEST {largest_text}\nEND\n"
        )
        try:
            with open(output_path, "w", encoding="utf-8") as output:
                output.write(report)
                output.flush()
        except OSError:
            raise OSError("report: cannot write")
        except ValueError:
            raise ValueError("report: invalid path")
    except ValueError as error:
        print("ERROR")
        print("ValueError")
        print(str(error))
    except OSError as error:
        print("ERROR")
        print("OSError")
        print(str(error))
    else:
        print("OK")
        print(checksum_text)


main()
