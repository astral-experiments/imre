import warnings


async def job():
    return 1


def forget():
    job()


with warnings.catch_warnings(record=True) as seen:
    warnings.simplefilter("always", RuntimeWarning)
    forget()
    assert len(seen) == 1
    assert seen[0].category is RuntimeWarning
    assert str(seen[0].message) == "coroutine 'job' was never awaited"

    pending = job()

    def get_pending():
        return pending

    get_pending()
    assert len(seen) == 1
    pending.close()
