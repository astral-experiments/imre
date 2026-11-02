//! Boxed hash handles share streaming state across Python references and native workers.
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    Context, Error, HashState, Id, NONE, Result, Value, buffer, builtins, data_argument, openssl,
};
use crate::Int;
use crate::objects::hash_hex;

pub(crate) const NAMES: &[&str] = &["md5", "sha1", "sha224", "sha256", "sha384", "sha512"];

#[derive(Clone)]
pub(crate) struct SharedHash(Arc<Mutex<State>>);

impl SharedHash {
    pub(crate) fn new(state: HashState) -> Self {
        Self::from_state(State::Portable(state))
    }

    fn from_state(state: State) -> Self {
        Self(Arc::new(Mutex::new(state)))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "the shared mutex already heap-allocates the portable hash state"
)]
pub(crate) enum State {
    Portable(HashState),
    OpenSsl(openssl::Stream),
}

impl State {
    pub(crate) fn class_name(&self) -> &'static str {
        match self {
            Self::Portable(hash) => hash.class_name(),
            Self::OpenSsl(_) => HashState::OPENSSL_CLASS,
        }
    }

    pub(crate) fn repr(&self, id: usize) -> impl fmt::Display {
        fmt::from_fn(move |f| match self {
            Self::Portable(hash) => write!(f, "{}", hash.repr(id)),
            Self::OpenSsl(hash) => {
                write!(f, "<{} {} object @ {id:#x}>", hash.name, self.class_name())
            }
        })
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Portable(hash) => hash.name(),
            Self::OpenSsl(hash) => hash.name,
        }
    }

    pub(crate) fn block_size(&self) -> usize {
        match self {
            Self::Portable(hash) => hash.block_size(),
            Self::OpenSsl(hash) => hash.block_size,
        }
    }

    pub(crate) fn digest_size(&self) -> usize {
        match self {
            Self::Portable(hash) => hash.digest_size(),
            Self::OpenSsl(hash) => hash.digest_size,
        }
    }

    pub(crate) fn is_xof(&self) -> bool {
        matches!(self, Self::Portable(hash) if hash.is_xof())
    }

    pub(super) fn copy(&self) -> Result<SharedHash> {
        Ok(SharedHash::from_state(match self {
            Self::Portable(hash) => Self::Portable(hash.clone()),
            Self::OpenSsl(hash) => Self::OpenSsl(hash.try_copy()?),
        }))
    }

    pub(crate) fn update(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Self::Portable(hash) => {
                hash.update(data);
                Ok(())
            }
            Self::OpenSsl(hash) => hash.update(data),
        }
    }

    pub(super) fn digest(&self, method: &str, length: Option<&Int>) -> Result<Value> {
        let hex = method == "hexdigest";
        Ok(match (self, length) {
            (Self::Portable(hash), Some(length)) if hex => {
                Value::Str(hash.xof_hexdigest(length)?.to_string())
            }
            (Self::Portable(hash), Some(length)) => Value::Bytes(hash.xof_digest(length)?),
            (Self::Portable(hash), None) if hex => Value::Str(hash.hexdigest()?.to_string()),
            (Self::Portable(hash), None) => {
                Value::Bytes(hash.fixed_digest(method, |bytes| Ok(bytes.to_vec()))?)
            }
            (Self::OpenSsl(hash), None) => {
                let bytes = hash.digest()?;
                if hex {
                    Value::Str(hash_hex(&bytes)?.to_string())
                } else {
                    Value::Bytes(bytes)
                }
            }
            (Self::OpenSsl(_), Some(_)) => unreachable!("fixed OpenSSL digest"),
        })
    }
}

pub(in crate::dynamic) fn construct(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (name, data, flag, string) = if method == "new" {
        let [name, data, flag, string] = builtins::bind_method_arguments(
            method,
            args,
            kwargs,
            ["name", "data", "usedforsecurity", "string"],
            2,
        )?;
        let id = name.ok_or_else(|| {
            Error::new(
                "TypeError",
                "new() missing required argument 'name' (pos 1)",
            )
        })?;
        let id = ctx.string_storage(id).ok_or_else(|| {
            let ty = if id == NONE {
                "None"
            } else {
                ctx.type_name(id)
            };
            Error::new(
                "TypeError",
                format!("new() argument 'name' must be str, not {ty}"),
            )
        })?;
        if matches!(ctx.heap.get(id), Value::SurrogateStr(_)) {
            ctx.encode_utf8_codepoints(id, false, "strict")?;
        }
        let name = ctx.str_value(id)?;
        if name.contains('\0') {
            return Err(Error::new("ValueError", "embedded null character"));
        }
        let canonical = HashState::provider(name)
            .ok()
            .map(|provider| provider.name)
            .filter(|name| NAMES.contains(name))
            .ok_or(id);
        (canonical, data, flag, string)
    } else {
        let name = NAMES
            .iter()
            .copied()
            .find(|&name| Some(name) == method.strip_prefix("openssl_"))
            .ok_or_else(|| Error::new("AttributeError", method))?;
        let [data, flag, string] = builtins::bind_method_arguments(
            method,
            args,
            kwargs,
            ["data", "usedforsecurity", "string"],
            1,
        )?;
        (Ok(name), data, flag, string)
    };
    let used = ctx.optional_truth(flag, true)?;
    let data = data_argument(data, string)?
        .map(|id| buffer(ctx, id))
        .transpose()?
        .unwrap_or_default();
    let provider = openssl::required()?;
    let lookup = match name {
        Ok(name) => name,
        Err(id) => ctx.str_value(id)?,
    };
    let digest = provider.digest_with_security(lookup, used)?;
    let name = name.map_err(|_| {
        Error::new(
            super::UNSUPPORTED_DIGEST,
            format!("unsupported hash type {lookup}"),
        )
    })?;
    let mut hash = openssl::Stream::new(name, &digest)?;
    if !data.is_empty() {
        hash.update(data)?;
    }
    Ok(ctx.alloc(Value::Hash(SharedHash::from_state(State::OpenSsl(hash)))))
}
