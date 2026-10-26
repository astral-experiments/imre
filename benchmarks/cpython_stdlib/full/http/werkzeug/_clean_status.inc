    def _clean_status(self, value: str | int | HTTPStatus) -> tuple[str, int]:
        if isinstance(value, (int, HTTPStatus)):
            status_code = int(value)
        else:
            value = value.strip()

            if not value:
                raise ValueError("Empty status argument")

            code_str, sep, _ = value.partition(" ")

            try:
                status_code = int(code_str)
            except ValueError:
                # only message
                return f"0 {value}", 0

            if sep:
                # code and message
                return value, status_code

        # only code, look up message
        try:
            status = f"{status_code} {HTTP_STATUS_CODES[status_code].upper()}"
        except KeyError:
            status = f"{status_code} UNKNOWN"

        return status, status_code
