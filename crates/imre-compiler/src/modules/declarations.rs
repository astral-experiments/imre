//! These sealed imports initialize no state observable in the native subset.
//! Reachable-only mode can bind them without implementing their operations;
//! their symbols still fail closed whenever an executable expression uses one.

use super::{Binding, Intrinsic, IntrinsicModule};

pub(super) fn module(name: &str) -> Option<&'static str> {
    [
        "configparser",
        "email.message",
        "email.parser",
        "email.policy",
        "importlib",
        "importlib.resources",
    ]
    .into_iter()
    .find(|&known| known == name)
}

pub(super) fn symbol(owner: IntrinsicModule, name: &str) -> Option<Binding> {
    use IntrinsicModule as M;
    let child = match (owner, name) {
        (M::Email, "message") => Some("email.message"),
        (M::Email, "parser") => Some("email.parser"),
        (M::Email, "policy") => Some("email.policy"),
        (M::Declaration("importlib"), "resources") => Some("importlib.resources"),
        _ => None,
    };
    if let Some(child) = child {
        return Some(Binding::IntrinsicModule(M::Declaration(child)));
    }
    let symbol = match (owner, name) {
        (M::Declaration("configparser"), "ConfigParser") => "configparser.ConfigParser",
        (M::Declaration("email.message"), "Message") => "email.message.Message",
        (M::Declaration("email.parser"), "FeedParser") => "email.parser.FeedParser",
        (M::Declaration("email.policy"), "compat32") => "email.policy.compat32",
        (M::Declaration("importlib.resources"), "files") => "importlib.resources.files",
        (M::Warnings, "warn") => "warnings.warn",
        (M::CollectionsAbc, "Collection") => "collections.abc.Collection",
        _ => return None,
    };
    Some(Binding::Intrinsic(Intrinsic::Declaration(symbol)))
}
