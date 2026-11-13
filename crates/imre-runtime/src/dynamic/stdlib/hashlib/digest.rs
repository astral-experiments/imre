//! Boxed hash handles share streaming state across Python references and native workers.
use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    Context, Error, HashState, Id, NONE, Result, Value, buffer, builtins, data_argument, openssl,
};
use crate::Int;
use crate::objects::{exceptions, hash_hex};

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
    reason = "both typed handles and boxed hash handles already heap-allocate the state"
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

    /// Select the same public provider as hashlib.new for a typed string and bytes.
    pub(crate) fn public(
        name: &str,
        data: &[u8],
        used_for_security: bool,
    ) -> exceptions::Result<Self> {
        let provider = HashState::provider(name)?;
        if provider.name != "md5" {
            return Ok(Self::Portable(
                provider.create(HashState::public_constructor(name), data),
            ));
        }
        if let Some(ssl) = openssl::provider() {
            match ssl
                .digest_with_security("md5", used_for_security)
                .and_then(|digest| Self::openssl("md5", &digest, data))
            {
                Ok(state) => return Ok(state),
                Err(error)
                    if !matches!(
                        error.kind.as_ref(),
                        super::UNSUPPORTED_DIGEST | "ValueError"
                    ) =>
                {
                    return Err(native_error(error));
                }
                Err(_) if !matches!(name, "md5" | "MD5") => {
                    return Err(exceptions::Error::new(
                        exceptions::Kind::ValueError,
                        format!("unsupported hash type {name}"),
                    ));
                }
                Err(_) => {}
            }
        }
        Ok(Self::Portable(
            provider.create(Some(super::md5::CLASS), data),
        ))
    }

    /// Initialize the EVP stream under the digest's selected OpenSSL policy.
    fn openssl(name: &'static str, digest: &openssl::Digest, data: &[u8]) -> Result<Self> {
        let mut hash = openssl::Stream::new(name, digest)?;
        if !data.is_empty() {
            hash.update(data)?;
        }
        Ok(Self::OpenSsl(hash))
    }

    /// Copy the streaming context without sharing subsequent writes.
    pub(crate) fn try_copy(&self) -> exceptions::Result<Self> {
        Ok(match self {
            Self::Portable(hash) => Self::Portable(hash.clone()),
            Self::OpenSsl(hash) => Self::OpenSsl(hash.try_copy().map_err(native_error)?),
        })
    }

    pub(super) fn copy(&self) -> Result<SharedHash> {
        Ok(SharedHash::from_state(self.try_copy()?))
    }

    /// Project native provider errors into the typed runtime's exception representation.
    pub(crate) fn update_native(&mut self, data: &[u8]) -> exceptions::Result<()> {
        self.update(data).map_err(native_error)
    }

    pub(in crate::dynamic) fn update(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Self::Portable(hash) => {
                hash.update(data);
                Ok(())
            }
            Self::OpenSsl(hash) => hash.update(data),
        }
    }

    /// Portable providers lend the snapshot bytes. OpenSSL hands over its output buffer.
    pub(crate) fn fixed_digest<T>(
        &self,
        method: &str,
        result: impl FnOnce(Cow<'_, [u8]>) -> exceptions::Result<T>,
    ) -> exceptions::Result<T> {
        match self {
            Self::Portable(hash) => hash.fixed_digest(method, |bytes| result(Cow::Borrowed(bytes))),
            Self::OpenSsl(hash) => result(Cow::Owned(hash.digest().map_err(native_error)?)),
        }
    }

    /// A runtime-selected algorithm may turn out to have a fixed output size.
    pub(crate) fn xof(&self, method: &str, length: &Int) -> exceptions::Result<Vec<u8>> {
        if let Self::Portable(hash) = self
            && hash.is_xof()
        {
            return hash.xof_digest(length);
        }
        Err(exceptions::Error::new(
            exceptions::Kind::TypeError,
            format!("HASH.{method}() takes no arguments (1 given)"),
        ))
    }

    pub(super) fn digest(&self, method: &str, length: Option<&Int>) -> Result<Value> {
        let result = |bytes: Cow<'_, [u8]>| {
            if method == "hexdigest" {
                hash_hex(&bytes).map(Value::Str)
            } else {
                Ok(Value::Bytes(bytes.into_owned()))
            }
        };
        Ok(match length {
            Some(length) => result(Cow::Owned(self.xof(method, length)?))?,
            None => self.fixed_digest(method, result)?,
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
    let state = State::openssl(name, &digest, data)?;
    Ok(ctx.alloc(Value::Hash(SharedHash::from_state(state))))
}

/// EVP stream operations report Python value or allocation errors.
fn native_error(error: Error) -> exceptions::Error {
    let kind = if error.kind == "MemoryError" {
        exceptions::Kind::MemoryError
    } else {
        exceptions::Kind::ValueError
    };
    exceptions::Error::new(kind, error.message)
}
