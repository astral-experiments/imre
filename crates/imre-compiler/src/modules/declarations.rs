//! Sealed import declarations and exports that require a real Python initializer.

use super::{Binding, Intrinsic, IntrinsicModule};

pub(super) fn module(name: &str) -> Option<&'static str> {
    ["configparser", "email.message", "email.policy"]
        .into_iter()
        .find(|&known| known == name)
}

pub(super) fn symbol(owner: IntrinsicModule, name: &str) -> Option<Binding> {
    use IntrinsicModule as M;
    match (owner, name) {
        (M::RuntimeSource("concurrent"), "futures") => {
            return Some(Binding::IntrinsicModule(M::RuntimeSource(
                "concurrent.futures",
            )));
        }
        (
            M::RuntimeSource(owner @ "concurrent.futures"),
            "FIRST_COMPLETED"
            | "FIRST_EXCEPTION"
            | "ALL_COMPLETED"
            | "CancelledError"
            | "TimeoutError"
            | "InvalidStateError"
            | "BrokenExecutor"
            | "Future"
            | "Executor"
            | "wait"
            | "as_completed"
            | "ProcessPoolExecutor"
            | "ThreadPoolExecutor",
        ) => {
            return Some(Binding::Intrinsic(Intrinsic::RuntimeImport(owner)));
        }
        _ => {}
    }
    let child = match (owner, name) {
        (M::Email, "message") => Some("email.message"),
        (M::Email, "policy") => Some("email.policy"),
        _ => None,
    };
    if let Some(child) = child {
        return Some(Binding::IntrinsicModule(M::Declaration(child)));
    }
    let symbol = match (owner, name) {
        (M::Declaration("configparser"), "ConfigParser") => "configparser.ConfigParser",
        (M::Declaration("email.message"), "Message") => "email.message.Message",
        (M::Declaration("email.policy"), "compat32") => "email.policy.compat32",
        _ => return None,
    };
    Some(Binding::Intrinsic(Intrinsic::Declaration(symbol)))
}
