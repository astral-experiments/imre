from uuid import UUID

def invoice_status(invoice_id: UUID) -> str:
    charged = {UUID(int=1): "settled"}
    try:
        return charged[invoice_id]
    except KeyError as exc:
        return "unrecognized invoice " + str(exc)

print(invoice_status(UUID("00112233-4455-6677-8899-aabbccddeeff")))
