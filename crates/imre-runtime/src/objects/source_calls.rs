//! Adapter from pooled checked signatures and exact native mappings to source binding.
use super::{
    Object,
    boxed_values::tuple_item,
    exceptions::{Error, Kind},
    mappings, object, records,
};
use crate::{
    Int, Str,
    source_arguments::{self, Shape},
};
use smallvec::SmallVec;

/// Bind an exact evaluated mapping into formal-number and remaining keyword dictionaries.
///
/// # Safety
/// The source, the two distinct fresh output dictionaries, and the literal tuples must
/// remain rooted. Outputs have the source value kind, with int and str keys respectively.
/// The compiler emits the signature header followed by fixed names and keyword defaults.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_bind_source_keywords(
    source: *const Object,
    slots: *const Object,
    rest: *const Object,
    qualified: *const Str,
    signature: *const usize,
    explicit: *const usize,
    positional: usize,
) {
    // SAFETY: tuple fields, strings and tagged small counts follow the checked call ABI.
    let bind = || unsafe {
        let item = |t, i| tuple_item(t as usize, i).1;
        let number = |i| Int::borrow(item(signature, i)).as_small().unwrap() as usize;
        let names = |t: *const usize, from| {
            (from..records::descriptor(t).length)
                .map(move |i| (&*(tuple_item(t as usize, i).1 as *const Str)).as_str())
        };
        let positional_only = number(0);
        let positional_count = number(1);
        let fixed_count = number(2);
        let positional_defaults = number(3);
        let varargs = item(signature, 4) != 0;
        let kwargs = item(signature, 5) != 0;
        let mut fields = SmallVec::<[&str; 8]>::new();
        fields
            .try_reserve(records::descriptor(signature).length - 6)
            .map_err(|_| Error::memory_error())?;
        fields.extend(names(signature, 6));
        let (fixed, kwdefaults) = fields.split_at(fixed_count);
        let (pos, kw) = fixed.split_at(positional_count);
        let shape = Shape {
            positional: pos,
            positional_only,
            keyword_only: kw,
            varargs,
            kwargs,
        };
        let given = Int::borrow(positional).as_small().unwrap() as usize;
        let qualified = (&*qualified).as_str();
        let name = qualified.rsplit('.').next().unwrap();
        object(source).0.with(|storage| {
            let dict = storage.dictionary();
            let mut keys = SmallVec::<[&str; 8]>::new();
            keys.try_reserve(records::descriptor(explicit).length)
                .map_err(|_| Error::memory_error())?;
            for key in names(explicit, 0) {
                keys.push(key);
            }
            let prefix = keys.len();
            for (key, _) in dict.string_entries() {
                if keys[..prefix].contains(&key.as_str()) {
                    return Err(Error::new(
                        Kind::TypeError,
                        format!("{qualified}() got multiple values for keyword argument '{key}'"),
                    ));
                }
                keys.try_reserve(1).map_err(|_| Error::memory_error())?;
                keys.push(key.as_str());
            }
            let bound = shape
                .bind(
                    name,
                    given,
                    keys.iter().copied(),
                    positional_defaults,
                    |k| kwdefaults.contains(&k),
                )
                .map_err(|err| match err {
                    source_arguments::Error::Type(msg) => Error::new(Kind::TypeError, msg),
                    source_arguments::Error::Memory => Error::memory_error(),
                })?;
            for ((key, value), slot) in dict
                .string_entries()
                .zip(bound.keywords.into_iter().skip(prefix))
            {
                if let Some(slot) = slot {
                    mappings::dict_set(slots, Int::from_usize(slot).into_raw(), value)?;
                } else {
                    mappings::dict_set(rest, std::ptr::from_ref(key) as usize, value)?;
                }
            }
            Ok(())
        })
    };
    if let Err(error) = bind() {
        error.raise();
    }
}
