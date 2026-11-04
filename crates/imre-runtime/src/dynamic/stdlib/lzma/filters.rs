//! Convert Python filter dictionaries to liblzma options and filter properties.
use std::ptr;

use liblzma::stream::{Filters, LzmaOptions, MatchFinder, Mode};
use liblzma_sys as sys;
use num_traits::{Signed, ToPrimitive};

use super::{Context, Error, Id, LZMA_ERROR, Result, Value, builtins, codec_error};

fn bad_options() -> Error {
    Error::new(LZMA_ERROR, "Invalid or unsupported options")
}

fn check(code: sys::lzma_ret) -> Result<()> {
    match code {
        sys::LZMA_OK => Ok(()),
        sys::LZMA_MEM_ERROR => Err(Error::new("MemoryError", "")),
        sys::LZMA_PROG_ERROR => Err(Error::new(LZMA_ERROR, "Internal error")),
        _ => Err(bad_options()),
    }
}

fn unsigned(ctx: &mut Context, value: Id, c_type: &str) -> Result<u64> {
    let number = ctx.index_integer(value)?;
    let number = number.big();
    if number.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    number.to_u64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!("Python int too large for C {c_type}"),
        )
    })
}

enum Options {
    Lzma(sys::lzma_options_lzma),
    Delta(u32),
    Bcj(sys::lzma_options_bcj),
}

struct Filter {
    id: u64,
    options: Options,
}

impl Filter {
    fn parse(ctx: &mut Context, spec: Id) -> Result<Self> {
        let entries = ctx
            .dict_storage(spec)
            .and_then(|dict| ctx.dict_entries(dict))
            .ok_or_else(|| {
                Error::new(
                    "TypeError",
                    "Filter specifier must be a dict or dict-like object",
                )
            })?
            .to_vec();
        let get = |name: &str| {
            entries
                .iter()
                .find_map(|&(key, value)| (ctx.str_value(key).ok() == Some(name)).then_some(value))
        };
        let id = get("id").ok_or_else(|| {
            Error::new("ValueError", "Filter specifier must have an \"id\" entry")
        })?;
        let preset = get("preset");
        let id = unsigned(ctx, id, "lzma_vli")?;
        let (mut options, kind) = match id {
            sys::LZMA_FILTER_LZMA1 | sys::LZMA_FILTER_LZMA2 => {
                let preset = match preset {
                    Some(p) => u32::try_from(unsigned(ctx, p, "uint32_t")?).map_err(|_| {
                        Error::new("OverflowError", "Python int too large for C uint32_t")
                    })?,
                    None => sys::LZMA_PRESET_DEFAULT,
                };
                // SAFETY: A zeroed options structure has valid integers and null pointers.
                // liblzma initializes the options for this preset before they are used.
                let mut options: sys::lzma_options_lzma = unsafe { std::mem::zeroed() };
                // SAFETY: options is a writable lzma_options_lzma.
                if unsafe { sys::lzma_lzma_preset(&mut options, preset) } != 0 {
                    return Err(Error::new(
                        LZMA_ERROR,
                        format!("Invalid compression preset: {preset}"),
                    ));
                }
                (Options::Lzma(options), "LZMA")
            }
            sys::LZMA_FILTER_DELTA => (Options::Delta(1), "delta"),
            sys::LZMA_FILTER_X86..=sys::LZMA_FILTER_SPARC => (
                Options::Bcj(sys::lzma_options_bcj { start_offset: 0 }),
                "BCJ",
            ),
            _ => return Err(Error::new("ValueError", format!("Invalid filter ID: {id}"))),
        };
        let invalid = || {
            Error::new(
                "ValueError",
                format!("Invalid filter specifier for {kind} filter"),
            )
        };
        for (key, value) in entries {
            let field = ctx.str_value(key).map_err(|_| invalid())?;
            let field = match (&mut options, field) {
                (_, "id") | (Options::Lzma(_), "preset") => continue,
                (Options::Lzma(o), "dict_size") => &mut o.dict_size,
                (Options::Lzma(o), "lc") => &mut o.lc,
                (Options::Lzma(o), "lp") => &mut o.lp,
                (Options::Lzma(o), "pb") => &mut o.pb,
                (Options::Lzma(o), "nice_len") => &mut o.nice_len,
                (Options::Lzma(o), "depth") => &mut o.depth,
                (Options::Lzma(o), "mode") => {
                    o.mode = option_number(ctx, value).ok_or_else(invalid)? as sys::lzma_mode;
                    continue;
                }
                (Options::Lzma(o), "mf") => {
                    o.mf = option_number(ctx, value).ok_or_else(invalid)? as sys::lzma_match_finder;
                    continue;
                }
                (Options::Delta(dist), "dist") => dist,
                (Options::Bcj(o), "start_offset") => &mut o.start_offset,
                _ => return Err(invalid()),
            };
            *field = option_number(ctx, value).ok_or_else(invalid)?;
        }
        Ok(Self { id, options })
    }

    fn encode(&self) -> Result<Vec<u8>> {
        let options = match &self.options {
            Options::Lzma(o) => ptr::from_ref(o).cast_mut().cast(),
            Options::Bcj(o) => ptr::from_ref(o).cast_mut().cast(),
            // The DELTA property is the distance minus one in a single byte.
            Options::Delta(dist) => {
                let byte = dist
                    .checked_sub(1)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(bad_options)?;
                return Ok(vec![byte]);
            }
        };
        let filter = sys::lzma_filter {
            id: self.id,
            options,
        };
        let mut size = 0;
        // SAFETY: options points at the C structure for this filter ID and stays live
        // for both calls.
        check(unsafe { sys::lzma_properties_size(&mut size, &filter) })?;
        let mut out = vec![0; size as usize];
        // SAFETY: out has exactly the size liblzma reported for this filter.
        check(unsafe { sys::lzma_properties_encode(&filter, out.as_mut_ptr()) })?;
        Ok(out)
    }

    fn append(&self, chain: &mut Filters, encode: bool) -> Result<()> {
        if let Options::Lzma(raw) = &self.options {
            let opts = stream_options(raw, encode)?;
            if self.id == sys::LZMA_FILTER_LZMA1 {
                chain.lzma1(&opts);
            } else {
                chain.lzma2(&opts);
            }
            return Ok(());
        }
        let props = self.encode()?;
        match self.id {
            sys::LZMA_FILTER_DELTA => chain.delta_properties(&props),
            sys::LZMA_FILTER_X86 => chain.x86_properties(&props),
            sys::LZMA_FILTER_POWERPC => chain.powerpc_properties(&props),
            sys::LZMA_FILTER_IA64 => chain.ia64_properties(&props),
            sys::LZMA_FILTER_ARM => chain.arm_properties(&props),
            sys::LZMA_FILTER_ARMTHUMB => chain.arm_thumb_properties(&props),
            sys::LZMA_FILTER_SPARC => chain.sparc_properties(&props),
            _ => unreachable!("validated filter ID"),
        }
        .map_err(codec_error)?;
        Ok(())
    }
}

fn option_number(ctx: &mut Context, value: Id) -> Option<u32> {
    ctx.index_integer(value).ok()?.big().to_u32()
}

fn stream_options(raw: &sys::lzma_options_lzma, encode: bool) -> Result<LzmaOptions> {
    let mut opts = LzmaOptions::new_preset(sys::LZMA_PRESET_DEFAULT).map_err(codec_error)?;
    opts.dict_size(raw.dict_size)
        .literal_context_bits(raw.lc)
        .literal_position_bits(raw.lp)
        .position_bits(raw.pb);
    if !encode {
        return Ok(opts);
    }
    opts.mode(match raw.mode {
        sys::LZMA_MODE_FAST => Mode::Fast,
        sys::LZMA_MODE_NORMAL => Mode::Normal,
        _ => return Err(bad_options()),
    })
    .nice_len(raw.nice_len)
    .match_finder(match raw.mf {
        sys::LZMA_MF_HC3 => MatchFinder::HashChain3,
        sys::LZMA_MF_HC4 => MatchFinder::HashChain4,
        sys::LZMA_MF_BT2 => MatchFinder::BinaryTree2,
        sys::LZMA_MF_BT3 => MatchFinder::BinaryTree3,
        sys::LZMA_MF_BT4 => MatchFinder::BinaryTree4,
        _ => return Err(bad_options()),
    })
    .depth(raw.depth);
    Ok(opts)
}

fn parse_chain(ctx: &mut Context, filters: Id) -> Result<Vec<Filter>> {
    let n = ctx.sequence_len(filters)?;
    if n > 4 {
        return Err(Error::new(
            "ValueError",
            "Too many filters - liblzma supports a maximum of 4",
        ));
    }
    (0..n)
        .map(|i| {
            let key = ctx.int(i as i64);
            let spec = ctx.get_item(filters, key)?;
            Filter::parse(ctx, spec)
        })
        .collect()
}

fn build_chain(ctx: &mut Context, filters: Id, encode: bool) -> Result<Filters> {
    let mut result = Filters::new();
    for filter in parse_chain(ctx, filters)? {
        filter.append(&mut result, encode)?;
    }
    Ok(result)
}

pub(super) fn chain(ctx: &mut Context, filters: Id) -> Result<Filters> {
    build_chain(ctx, filters, true)
}

pub(super) fn decoder_chain(ctx: &mut Context, filters: Id) -> Result<Filters> {
    build_chain(ctx, filters, false)
}

pub(super) fn alone(ctx: &mut Context, filters: Id) -> Result<LzmaOptions> {
    let filters = parse_chain(ctx, filters)?;
    if let [
        Filter {
            id: sys::LZMA_FILTER_LZMA1,
            options: Options::Lzma(raw),
        },
    ] = filters.as_slice()
    {
        return stream_options(raw, true);
    }
    Err(Error::new(
        "ValueError",
        "Invalid filter chain for FORMAT_ALONE - must be a single LZMA1 filter",
    ))
}

pub(in crate::dynamic) fn properties(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_encode_filter_properties" {
        let spec = builtins::one_positional_arg(name, args, kwargs)?;
        let bytes = Filter::parse(ctx, spec)?.encode()?;
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    let [filter_id, data] = args else {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes exactly 2 arguments ({} given)", args.len()),
        ));
    };
    let id = unsigned(ctx, *filter_id, "lzma_vli")?;
    let fields = super::streams::with_input(ctx, *data, |ctx, view| {
        decode(id, ctx.native_buffer_bytes(view)?)
    })?;
    let mut entries = vec![(ctx.text("id"), ctx.int(id as i64))];
    for (key, value) in fields {
        entries.push((ctx.text(key), ctx.int(i64::from(value))));
    }
    Ok(ctx.dict(entries))
}

fn decode(id: u64, data: &[u8]) -> Result<Vec<(&'static str, u32)>> {
    if id == sys::LZMA_FILTER_DELTA {
        let [distance] = data else {
            return Err(bad_options());
        };
        return Ok(vec![("dist", u32::from(*distance) + 1)]);
    }
    let mut filter = sys::lzma_filter {
        id,
        options: ptr::null_mut(),
    };
    // SAFETY: liblzma receives the complete property buffer and its length.
    // On success it allocated options of the type corresponding to this ID.
    check(unsafe {
        sys::lzma_properties_decode(&mut filter, ptr::null(), data.as_ptr(), data.len())
    })?;
    let fields = match id {
        sys::LZMA_FILTER_LZMA1 | sys::LZMA_FILTER_LZMA2 => {
            let opts = filter.options.cast::<sys::lzma_options_lzma>();
            let mut fields = Vec::new();
            // SAFETY: liblzma returns a non-null allocation and initializes the
            // properties of this filter ID. Encoder-only fields may be uninitialized.
            unsafe {
                if id == sys::LZMA_FILTER_LZMA1 {
                    fields.extend([("lc", (*opts).lc), ("lp", (*opts).lp), ("pb", (*opts).pb)]);
                }
                fields.push(("dict_size", (*opts).dict_size));
            }
            Ok(fields)
        }
        sys::LZMA_FILTER_X86..=sys::LZMA_FILTER_SPARC => {
            // SAFETY: A BCJ filter may return null to indicate the default offset.
            let opts = unsafe { filter.options.cast::<sys::lzma_options_bcj>().as_ref() };
            Ok(opts
                .map(|o| vec![("start_offset", o.start_offset)])
                .unwrap_or_default())
        }
        _ => Err(Error::new("ValueError", format!("Invalid filter ID: {id}"))),
    };
    let mut filters = [
        filter,
        sys::lzma_filter {
            id: sys::LZMA_VLI_UNKNOWN,
            options: ptr::null_mut(),
        },
    ];
    // SAFETY: A terminated chain of options allocated using liblzma's default allocator.
    unsafe { sys::lzma_filters_free(filters.as_mut_ptr(), ptr::null()) };
    fields
}
