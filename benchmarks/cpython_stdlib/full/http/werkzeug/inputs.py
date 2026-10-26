def exercise(response: Response) -> None:
    print(response._clean_status(HTTPStatus.OK))
    print(response._clean_status(HTTPStatus.NOT_FOUND))
    print(response._clean_status(HTTPStatus.CONTENT_TOO_LARGE))
    print(response._clean_status(599))
    for text in ["  404  ", "201 custom", "message only", "999", "0", "200  two spaces"]:
        print(response._clean_status(text))
    try:
        response._clean_status("  ")
    except ValueError as error:
        print(str(error))


exercise(Response())
