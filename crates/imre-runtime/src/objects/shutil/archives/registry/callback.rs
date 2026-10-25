//! Bind saved keywords at unpack time. The original objects remain rooted;
//! no collection or registry borrow spans a call back into user code.
use super::*;
use crate::objects::{mappings, records};
use crate::{Bytes, Int, List};
use std::sync::atomic::{AtomicU8, Ordering};

pub(in super::super) struct Callback {
    function: OwnedWord,
    adapter: *const (),
    frame: *const RecordDescriptor,
    metadata: OwnedWord,
    extras: OwnedWord,
    nonstring_keys: bool,
    keyword_value_kind: Option<WordKind>,
    pub(super) root_flag: Option<&'static AtomicU8>,
}

fn type_error(message: impl Into<Str>) -> Error {
    Error::new(Kind::TypeError, message)
}

unsafe fn field(tuple: usize, index: usize) -> OwnedWord {
    let p = tuple as *const usize;
    // SAFETY: the caller checks the index and keeps the parent alive.
    unsafe {
        let kind = WordKind::new(*records::descriptor(p).kinds.add(index));
        kind.acquire(kind.read_record_field(p.add(index)))
    }
}

fn text(word: &OwnedWord) -> &str {
    debug_assert_eq!(word.kind.raw(), 3);
    // SAFETY: the owner has an exact string descriptor.
    unsafe { (&*(word.word as *const Str)).as_str() }
}

fn string_word(s: StrOwner) -> OwnedWord {
    // SAFETY: transfer one owner of this exact string.
    unsafe { OwnedWord::from_owned(StrOwner::into_raw(s) as usize, WordKind::new(3)) }
}

fn items(v: &OwnedWord) -> Option<Vec<OwnedWord>> {
    // SAFETY: each returned element owns its reference before the storage borrow ends.
    unsafe {
        match v.kind.raw() {
            3 => Some(
                text(v)
                    .chars()
                    .map(|ch| string_word(StrOwner::new(Str::from(ch.to_string()))))
                    .collect(),
            ),
            4 => Some(
                (*(v.word as *const List))
                    .0
                    .with(|ints| ints.iter().map(|n| WordKind::new(2).acquire(n.0)).collect()),
            ),
            5 => (*(v.word as *const Object)).0.with(|s| {
                if let Storage::Sequence(words) = s {
                    let words = words.resolved();
                    Some(
                        words
                            .values
                            .iter()
                            .map(|&n| words.kind.acquire(n))
                            .collect(),
                    )
                } else {
                    None
                }
            }),
            6 => {
                let desc = records::descriptor(v.word as *const usize);
                desc.semantics
                    .as_ref()
                    .filter(|s| s.kind == 1)
                    .map(|_| (0..desc.length).map(|i| field(v.word, i)).collect())
            }
            8 => Some(
                (&*(v.word as *const Bytes))
                    .iter()
                    .map(|&n| {
                        OwnedWord::from_owned(
                            Int::from_usize(usize::from(n)).into_raw(),
                            WordKind::new(2),
                        )
                    })
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// Convert bare and optional scalars without reinterpreting pointer storage.
fn stored(v: OwnedWord, want: WordKind) -> Result<OwnedWord> {
    if v.kind == want {
        return Ok(v);
    }
    let from = v.kind.raw();
    let to = want.raw();
    if to & 256 != 0 && (from == 0 || from == to & !256) {
        let n = match from {
            0 => 0,
            1 => v.word + 1,
            7 => Rc::into_raw(Rc::new(f64::from_bits(v.word as u64))) as usize,
            _ => v.into_word(),
        };
        // SAFETY: the cases above construct the declared nullable encoding.
        return Ok(unsafe { OwnedWord::from_owned(n, want) });
    }
    if from & 256 != 0 && v.word != 0 && to == from & !256 {
        // SAFETY: a present optional float owns a live f64; reference kinds
        // use the same payload allocation in both representations.
        let n = match to {
            1 => v.word - 1,
            7 => unsafe { (*(v.word as *const f64)).to_bits() as usize },
            _ => v.into_word(),
        };
        return Ok(unsafe { OwnedWord::from_owned(n, want) });
    }
    Err(type_error(
        "archive callback argument does not match its native parameter type",
    ))
}

impl Callback {
    /// # Safety
    /// The compiler supplies matching live records and a fixed native adapter.
    pub(super) unsafe fn new(
        function: *const usize,
        adapter: *const (),
        frame: *const RecordDescriptor,
        metadata: *const usize,
        extras: usize,
        extras_kind: u32,
        rest_kind: u32,
    ) -> Self {
        // SAFETY: acquire the original values; the adapter and its layout are static.
        unsafe {
            Self {
                function: WordKind::new(6).acquire(function as usize),
                adapter,
                frame,
                metadata: WordKind::new(6).acquire(metadata as usize),
                extras: WordKind::new(extras_kind & 0xffff).acquire(extras),
                nonstring_keys: extras_kind & 0x10000 != 0,
                keyword_value_kind: (rest_kind != u32::MAX).then(|| WordKind::new(rest_kind)),
                root_flag: None,
            }
        }
    }

    fn keywords(&self) -> Result<Vec<(String, OwnedWord)>> {
        if self.extras.kind.raw() == 0 {
            return Ok(vec![]);
        }
        if self.extras.kind.raw() == 5 {
            let object = self.extras.word as *const Object;
            if self.nonstring_keys {
                // SAFETY: only a dictionary with non-string keys has this flag.
                return if unsafe { Int(mappings::imre_dict_len(object)) }.as_small() == Some(0) {
                    Ok(vec![])
                } else {
                    Err(type_error("keywords must be strings"))
                };
            }
            // SAFETY: each value is acquired while the mapping remains rooted.
            let mapping = unsafe {
                (&*object).0.with(|s| {
                    if let Storage::Dictionary(m) = s {
                        Some(Ok(m.owned_string_entries()))
                    } else {
                        None
                    }
                })
            };
            if let Some(mapping) = mapping {
                return mapping;
            }
        }
        let rows = items(&self.extras).ok_or_else(|| {
            let ty = match self.extras.kind.raw() {
                1 => "bool",
                2 => "int",
                7 => "float",
                _ => "object",
            };
            type_error(format!("'{ty}' object is not iterable"))
        })?;
        let mut pairs = Vec::<(String, OwnedWord)>::new();
        for (i, row) in rows.into_iter().enumerate() {
            let mut pair = items(&row).ok_or_else(|| {
                type_error(format!(
                    "cannot convert dictionary update sequence element #{i} to a sequence"
                ))
            })?;
            if pair.len() != 2 {
                return Err(Error::new(
                    Kind::ValueError,
                    format!(
                        "dictionary update sequence element #{i} has length {}; 2 is required",
                        pair.len()
                    ),
                ));
            }
            let value = pair.pop().unwrap();
            let key = pair.pop().unwrap();
            if key.kind.raw() != 3 {
                return Err(type_error("keywords must be strings"));
            }
            let name = text(&key);
            if let Some((_, old)) = pairs.iter_mut().find(|(key, _)| key == name) {
                *old = value;
            } else {
                pairs.push((name.into(), value));
            }
        }
        Ok(pairs)
    }

    pub(in super::super) fn call(
        &self,
        filename: StrOwner,
        dest: StrOwner,
        filter: Option<StrOwner>,
        explicit: bool,
    ) -> Result<()> {
        let mut kwargs = self.keywords()?;
        if let Some(filter) = filter {
            let value = string_word(filter);
            if let Some((_, old)) = kwargs.iter_mut().find(|(k, _)| k == "filter") {
                if explicit {
                    return Err(type_error(format!(
                        "{}() got multiple values for keyword argument 'filter'",
                        text(&unsafe { field(self.metadata.word, 0) })
                    )));
                }
                *old = value;
            } else {
                kwargs.push(("filter".into(), value));
            }
        }
        let frame = self.prepare([string_word(filename), string_word(dest)], kwargs)?;
        type Unpack = unsafe extern "C" fn(*const (), *const usize, *const usize) -> u8;
        // SAFETY: the unpack registry installs exactly this adapter ABI and owns the callable.
        unsafe {
            std::mem::transmute::<*const (), Unpack>(self.adapter)(
                std::ptr::null(),
                self.function.word as *const usize,
                frame.word as *const usize,
            );
        }
        Ok(())
    }

    pub(super) fn check_pack_extras(&self) -> Result<()> {
        if self.extras.kind.raw() == 0 {
            return Ok(());
        }
        fn entries(v: &OwnedWord) -> Option<Vec<OwnedWord>> {
            matches!(v.kind.raw(), 4..=6).then(|| items(v)).flatten()
        }
        let rows =
            entries(&self.extras).ok_or_else(|| type_error("extra_args needs to be a sequence"))?;
        for row in rows {
            if entries(&row).is_none_or(|parts| parts.len() != 2) {
                return Err(type_error("extra_args elements are : (arg_name, value)"));
            }
        }
        Ok(())
    }

    pub(in super::super) fn supports_root_dir(&self) -> bool {
        self.root_flag
            .is_some_and(|flag| flag.load(Ordering::Relaxed) != 0)
    }

    pub(in super::super) fn pack(
        &self,
        base: StrOwner,
        dir: StrOwner,
        dry: OwnedWord,
        owner: Option<&str>,
        group: Option<&str>,
        root: Option<StrOwner>,
    ) -> Result<Option<StrOwner>> {
        fn nullable(value: Option<&str>) -> OwnedWord {
            value.map_or_else(
                || unsafe { OwnedWord::from_owned(0, WordKind::new(0)) },
                |s| string_word(StrOwner::new(Str::from(s))),
            )
        }
        let mut kwargs = vec![
            ("dry_run".to_string(), dry),
            ("logger".into(), nullable(None)),
            ("owner".into(), nullable(owner)),
            ("group".into(), nullable(group)),
        ];
        let mut assign = |name: String, value: OwnedWord| {
            if let Some((_, old)) = kwargs.iter_mut().find(|(key, _)| *key == name) {
                *old = value;
            } else {
                kwargs.push((name, value));
            }
        };
        for (name, value) in self.keywords()? {
            assign(name, value);
        }
        if let Some(root) = root {
            assign("root_dir".into(), string_word(root));
        }
        let frame = self.prepare([string_word(base), string_word(dir)], kwargs)?;
        type Pack = unsafe extern "C" fn(*const (), *const usize, *const usize) -> *const Str;
        // SAFETY: packing adapters return an owned string or a null sentinel on a pending exception.
        unsafe {
            let result = std::mem::transmute::<*const (), Pack>(self.adapter)(
                std::ptr::null(),
                self.function.word as *const usize,
                frame.word as *const usize,
            );
            Ok((!result.is_null()).then(|| StrOwner::from_raw(result)))
        }
    }

    fn prepare(
        &self,
        positional_values: [OwnedWord; 2],
        kwargs: Vec<(String, OwnedWord)>,
    ) -> Result<OwnedWord> {
        // SAFETY: immutable metadata holds two names and one fixed tuple per
        // native parameter. Its corresponding frame descriptor is static.
        let (short, params) = unsafe {
            let len = records::descriptor(self.metadata.word as *const usize).length;
            (
                field(self.metadata.word, 1),
                (2..len)
                    .map(|i| {
                        let p = field(self.metadata.word, i);
                        (
                            field(p.word, 0),
                            field(p.word, 1),
                            (field(p.word, 2).word != 0).then(|| field(p.word, 3)),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let short = text(&short);
        let mut bound: Vec<Option<OwnedWord>> = (0..params.len()).map(|_| None).collect();
        let positional = params
            .iter()
            .take_while(|(_, mode, _)| matches!(text(mode), "/" | ""))
            .count();
        for (i, value) in positional_values.into_iter().enumerate().take(positional) {
            bound[i] = Some(value);
        }
        let rest = self.keyword_value_kind.map(|kind| {
            let index = params
                .iter()
                .position(|(_, mode, _)| text(mode) == "**")
                .expect("keyword rest slot");
            let dict = mappings::imre_dict_new(3, kind.raw());
            // SAFETY: this new dictionary is owned by the argument frame for this call.
            bound[index] = Some(unsafe { OwnedWord::from_owned(dict as usize, WordKind::new(5)) });
            (dict, kind)
        });
        for (key, value) in kwargs {
            if let Some(index) = params
                .iter()
                .position(|(n, mode, _)| text(n) == key && matches!(text(mode), "" | "*"))
            {
                if bound[index].replace(value).is_some() {
                    return Err(type_error(format!(
                        "{short}() got multiple values for argument '{key}'"
                    )));
                }
            } else if let Some((dict, kind)) = rest {
                let key = string_word(StrOwner::new(Str::from(key)));
                let value = stored(value, kind)?;
                // SAFETY: the frame roots the fresh dictionary; both operands have its declared kinds.
                unsafe { mappings::dict_set(dict, key.word, value.word)? };
            } else {
                return Err(type_error(format!(
                    "{short}() got an unexpected keyword argument '{key}'"
                )));
            }
        }
        if positional < 2 {
            let plural = if positional == 1 { "" } else { "s" };
            return Err(type_error(format!(
                "{short}() takes {positional} positional argument{plural} but 2 were given"
            )));
        }
        for mode in [false, true] {
            let missing: Vec<String> = params
                .iter()
                .enumerate()
                .filter(|(i, (_, kind, default))| {
                    (text(kind) == "*") == mode && default.is_none() && bound[*i].is_none()
                })
                .map(|(_, (n, _, _))| format!("'{}'", text(n)))
                .collect();
            if !missing.is_empty() {
                let label = if mode { "keyword-only" } else { "positional" };
                let count = missing.len();
                let plural = if count == 1 { "" } else { "s" };
                let names = match missing.as_slice() {
                    [a] => a.clone(),
                    [a, b] => format!("{a} and {b}"),
                    many => format!("{}, and {}", many[..count - 1].join(", "), many[count - 1]),
                };
                return Err(type_error(format!(
                    "{short}() missing {count} required {label} argument{plural}: {names}"
                )));
            }
        }
        // SAFETY: the frame holds precisely the declared arguments. The original
        // callable is rooted separately and survives a callback removing itself.
        unsafe {
            let frame = allocate_record(self.frame);
            if frame.is_null() {
                return Err(Error::allocation());
            }
            let owner = OwnedWord::from_owned(frame as usize, WordKind::new(6));
            for (i, (v, (_, _, default))) in bound.into_iter().zip(params).enumerate() {
                let kind = WordKind::new(*(*self.frame).kinds.add(i));
                stored(v.or(default).unwrap(), kind)?.initialize_record_field(frame.add(i));
            }
            Ok(owner)
        }
    }
}
