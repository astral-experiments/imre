"""Join the TCP observations to every occurrence in the stock Linux suite."""
import ast
import hashlib

from suite_evidence import build_report, reference_cases, roles
from suite_inventory import cases, load_suite

# Three skips arise inside tests or fixture setup, rather than a decorator.
# These locations and conditions refer to the pinned test_socket.py.
RUNTIME_SKIPS = {
    "network interface `vcan0` does not exist": {
        "CANTest": (343, "binding the CAN socket to self.interface raised OSError"),
        "ISOTPTest": (2419, "binding the CAN_ISOTP socket raised ENODEV"),
        "J1939Test": (2481, "binding the CAN_J1939 socket raised ENODEV"),
    },
    "test should be run as root": {
        "GeneralModuleTests": (1153, "socket.sethostname('new') raised EPERM"),
    },
    "('[Errno 2] No such file or directory', 'aead', 'gcm(aes)')": {
        "LinuxKernelCryptoAPI": (7010, "AF_ALG bind for ('aead', 'gcm(aes)') raised FileNotFoundError"),
    },
}


def skip_condition(case, reason, classes):
    if reason in RUNTIME_SKIPS:
        return RUNTIME_SKIPS[reason][type(case).__name__]
    for owner in type(case).__mro__:
        cls = classes.get(owner.__name__)
        if cls is None:
            continue
        method = next((node for node in cls.body if isinstance(node, ast.FunctionDef)
                       and node.name == case._testMethodName), None)
        for node in (method, cls):
            for decorator in node.decorator_list if node else []:
                if isinstance(decorator, ast.Call) and len(decorator.args) >= 2:
                    condition, message = decorator.args[:2]
                    if isinstance(message, ast.Constant) and message.value == reason:
                        return decorator.lineno, ast.unparse(decorator.func) + "(" + ast.unparse(condition) + ")"
    raise ValueError(f"{case.id()}: unmatched stock skip {reason!r}")


def report(reference, path, bodies, tcp, backlog, options, descriptors, level):
    discovered = list(cases(load_suite(reference["module"])))
    variants = [(c.id(), f"{type(c).__module__}.{type(c).__qualname__}") for c in discovered]
    refs = reference_cases(reference)
    classes = {node.name: node for node in ast.parse(path.read_bytes()).body if isinstance(node, ast.ClassDef)}
    exceptions = {}
    for case, (name, variant) in zip(discovered, variants, strict=True):
        ref = refs[name, 1]  # The checked source pin has no repeated IDs.
        if isinstance(ref, dict) and "skipped" in ref:
            reason = ref["skipped"]
            line, condition = skip_condition(case, reason, classes)
            exceptions[name, 1, level] = {
                "kind": "upstream-skip", "platform": reference["platform"], "fixture_variant": variant,
                "configuration": "stock CPython 3.14.5, Linux, regrtest --single-process --randseed 1",
                "reason": reason, "condition": condition, "source": f"Lib/test/test_socket.py:{line}",
            }
    observations = {}
    selected = [(owner, method) for owner in ("BasicTCPTest", "BasicTCPTest2") for method in tcp]
    selected += [("GeneralModuleTests", method) for method in (backlog, *options)]
    selected += [("InheritanceTest", method) for method in descriptors]
    for owner, method in selected:
        name = f"{reference['module']}.{owner}.{method}"
        host = method in options
        if host:
            fixture = "Original CPython TestCase, module lifecycle and addCleanup; pipe forwards calls to one native socket."
            operation = "Native AF_INET socket, SO_REUSEADDR and close; incorrect initial/write value rejected by stock assertion."
        elif method in descriptors:
            fixture = "Original host module setup/cleanup; native TestCase.assertTrue compares the original bool operands."
            operation = {
                "test_default_inheritable": "Native socket's descriptor starts non-inheritable.",
                "test_set_inheritable": "Original method changes and reads the real descriptor flag in both directions.",
                "test_dup": "Original method duplicates the native socket, closes the original and checks the independent, non-inheritable descriptor.",
            }[method]
        elif method in tcp:
            fixture = ("Original CPython module, threaded class setup/teardown, connected endpoints and cleanups. "
                       "Each child duplicates its fixture fd; both original peer methods and bytes equality run natively.")
            operation = "Original sender and receiver call native TCP send/recv/sendall; corrupt peer tripped the original assertion."
            if owner == "BasicTCPTest2":
                operation += " Its NetworkConnectionTest variant specifically exercises socket.create_connection on CPython."
        else:
            fixture = "Original host module setup/cleanup; native TestCase instance and context managers."
            operation = "Original body creates, binds, listens and closes native sockets at backlogs zero, negative one and default."
        proof = {"fixture": fixture, "operation": operation, "opt_level": level,
                 "method_sha256": hashlib.sha256(bodies[method]).hexdigest()}
        if method in tcp:
            proof["companion_sha256"] = hashlib.sha256(bodies["_" + method]).hexdigest()
        observations[name, 1] = {
            "outcome": "pass", "evidence": proof,
            "roles": roles(body="cpython" if host else "imre",
                           assertions="cpython" if host else "none" if method == backlog else "imre",
                           operation="mixed" if owner == "BasicTCPTest2" else "imre", fixture="mixed"),
        }
    return build_report(reference, level, variants, observations, exceptions=exceptions)
