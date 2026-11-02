import multiprocessing
import os
import pickle
from multiprocessing import process, reduction


def current():
    assert multiprocessing.current_process() is process.current_process()
    assert multiprocessing.current_process().pid == os.getpid()
    spawn = multiprocessing.get_context("spawn")
    assert spawn is multiprocessing.get_context("spawn")
    assert spawn.get_start_method() == "spawn"
    assert reduction.ForkingPickler.__bases__ == (pickle.Pickler,)
    with reduction.ForkingPickler.dumps(42, 2) as packet:
        assert bytes(packet) == b"\x80\x02K*."
        packet[3] = 43
        assert reduction.ForkingPickler.loads(packet) == 43
    try:
        multiprocessing.get_context("missing-start-method")
    except ValueError:
        pass
    else:
        raise AssertionError("accepted an unknown start method")
    print("multiprocessing context")
