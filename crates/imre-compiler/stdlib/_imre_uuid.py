def _swap_fields(data):
    # Little-endian UUIDs reverse the first three fields.
    return data[3::-1] + data[5:3:-1] + data[7:5:-1] + data[8:]


def _format_uuid(hex):
    return f'{hex[:8]}-{hex[8:12]}-{hex[12:16]}-{hex[16:20]}-{hex[20:]}'
