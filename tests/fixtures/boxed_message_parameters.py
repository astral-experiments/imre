from email.message import Message


def current():
    message = Message()
    assert message.get_params("missing") == "missing"
    message["Content-Disposition"] = (
        "attachment; filename*1*=a%20b; filename*0*=utf-8'en'caf%C3%A9; "
        'title="a;b"; bare; ñ*=ordinary'
    )
    print(message.get_params(header="Content-Disposition"))
    print(message.get_params(header="Content-Disposition", unquote=False))
