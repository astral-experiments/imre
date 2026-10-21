from sys import argv

from .compare import alert, latest, summarize, within
from .load import align, load, number
from .model import Sample, Summary


def decimal(value: int) -> str:
    text = str(value)
    digits = len(text) - (1 if value < 0 else 0)
    if digits > 4300:
        raise ValueError(
            "Exceeds the limit (4300 digits) for integer string conversion; "
            "use sys.set_int_max_str_digits() to increase the limit"
        )
    return text


def timestamp_text(timestamp: int | None) -> str:
    if timestamp is None:
        return "None"
    return decimal(timestamp)


def render(summary: Summary) -> str:
    newest = timestamp_text(summary.latest)
    signed_error = decimal(summary.signed_error)
    absolute_error = decimal(summary.absolute_error)
    squared_error = decimal(summary.squared_error)
    return (
        "TELEMETRY\n"
        f"SAMPLES {summary.samples}\n"
        f"SIGNED_ERROR {signed_error}\n"
        f"ABSOLUTE_ERROR {absolute_error}\n"
        f"SQUARED_ERROR {squared_error}\n"
        f"ALERT {summary.alert}\n"
        f"WITHIN {summary.within}\n"
        f"LATEST {newest}\n"
        "END\n"
    )


def query(
    operation: str,
    reference: list[Sample],
    observed: list[Sample],
    threshold: int,
    repetitions: int,
) -> tuple[str, int]:
    checksum = 0
    result = ""
    if operation == "report":
        summary = summarize(reference, observed, threshold)
        checksum = summary.signed_error + summary.absolute_error + summary.squared_error
        for iteration in range(1, repetitions):
            summary = summarize(reference, observed, threshold)
            checksum += (
                summary.signed_error + summary.absolute_error + summary.squared_error
            )
        result = render(summary)
    elif operation == "alert" or operation == "within":
        flag = False
        for iteration in range(repetitions):
            if operation == "alert":
                flag = alert(reference, observed, threshold)
            else:
                flag = within(reference, observed, threshold)
            if flag:
                checksum += 1
        result = f"{flag}\n"
    elif operation == "latest":
        timestamp: int | None = None
        for iteration in range(repetitions):
            timestamp = latest(reference, observed, threshold)
            if timestamp is not None:
                checksum += timestamp
        result = timestamp_text(timestamp) + "\n"
    else:
        raise ValueError("unknown telemetry operation")
    return result, checksum


def main() -> None:
    try:
        if len(argv) != 7:
            raise ValueError(
                "usage: telemetry_comparison MODE REFERENCE OBSERVED REPORT THRESHOLD REPEATS"
            )
        _, operation, reference_path, observed_path, output_path, limit, repeats = argv
        threshold = number(limit, "threshold")
        repetitions = number(repeats, "repetitions")
        if threshold < 0:
            raise ValueError("threshold must be nonnegative")
        if repetitions < 1:
            raise ValueError("repetitions must be positive")
        reference = load(reference_path, "reference")
        observed = load(observed_path, "observed")
        align(reference, observed)
        result, checksum = query(operation, reference, observed, threshold, repetitions)
        checksum_text = decimal(checksum)
        # Both inputs are closed before opening output, including path aliases.
        try:
            with open(output_path, "w", encoding="utf-8") as output:
                output.write(result)
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
