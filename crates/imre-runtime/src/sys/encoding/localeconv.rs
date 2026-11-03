//! Owned values from the C library's numeric and monetary conventions.
use super::{decode_locale, with_ctype_locale, with_process_locale};
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;

pub(crate) struct NumericLocale {
    pub(crate) decimal_point: String,
    pub(crate) thousands_sep: String,
    pub(crate) grouping: Vec<u8>,
}

pub(crate) enum Convention {
    Text(String),
    Grouping(Vec<u8>),
    Number(i64),
}

/// Copy through the first zero (repeat) or CHAR_MAX (stop), except for an empty pattern.
unsafe fn copy_grouping(mut ptr: *const libc::c_char) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        // SAFETY: callers pass a grouping from a locked lconv; both terminators
        // are checked before advancing past this element.
        let size = unsafe { *ptr };
        if size != 0 || !result.is_empty() {
            result.push(size as u8);
        }
        if size == 0 || size == libc::c_char::MAX {
            return result;
        }
        // SAFETY: this was a width, so libc provides another grouping byte.
        ptr = unsafe { ptr.add(1) };
    }
}

fn category_text<const N: usize>(
    category: libc::c_int,
    fields: [CString; N],
) -> crate::Result<[String; N]> {
    let decode = || {
        let mut strings = std::array::from_fn(|_| String::new());
        for (field, string) in fields.iter().zip(&mut strings) {
            *string = decode_locale(field)?;
        }
        Ok(strings)
    };
    // CPython tests all fields in the category together. Empty monetary fields
    // request the monetary encoding; empty numeric separators alone do not.
    let switch_ctype = fields.iter().any(|field| match field.as_bytes() {
        [] => category == libc::LC_MONETARY,
        [byte] => !byte.is_ascii(),
        _ => true,
    });
    if switch_ctype {
        // SAFETY: these queries run under the locale lock. A later setlocale
        // query may invalidate its previous result, so copy the character name first.
        let current = unsafe { libc::setlocale(libc::LC_CTYPE, std::ptr::null()) };
        let current = (!current.is_null()).then(|| {
            // SAFETY: the successful query returned a NUL-terminated locale name.
            unsafe { CStr::from_ptr(current) }.to_owned()
        });
        // SAFETY: a null name queries the selected category under the same lock.
        let name = unsafe { libc::setlocale(category, std::ptr::null()) };
        if !name.is_null() {
            // SAFETY: with_ctype_locale copies this name before any locale operation.
            let name = unsafe { CStr::from_ptr(name) };
            if current.as_deref() != Some(name)
                && let Some(result) = with_ctype_locale(OsStr::from_bytes(name.to_bytes()), decode)
            {
                return result;
            }
        }
    }
    decode()
}

struct RawNumeric {
    text: [CString; 2],
    grouping: Vec<u8>,
}

impl RawNumeric {
    /// The caller holds the locale lock and supplies the current libc lconv.
    unsafe fn copy(lc: &libc::lconv) -> Self {
        // SAFETY: lconv's strings remain valid until the next locale operation.
        unsafe {
            Self {
                text: [lc.decimal_point, lc.thousands_sep]
                    .map(|ptr| CStr::from_ptr(ptr).to_owned()),
                grouping: copy_grouping(lc.grouping),
            }
        }
    }

    fn decode(self) -> crate::Result<NumericLocale> {
        let [decimal_point, thousands_sep] = category_text(libc::LC_NUMERIC, self.text)?;
        Ok(NumericLocale {
            decimal_point,
            thousands_sep,
            grouping: self.grouping,
        })
    }
}

pub(crate) fn numeric_locale() -> crate::Result<NumericLocale> {
    with_process_locale(|| {
        // SAFETY: ISO C supplies a valid lconv. Copy before querying any category.
        let numeric = unsafe { RawNumeric::copy(&*libc::localeconv()) };
        numeric.decode()
    })
}

pub(crate) fn locale_conventions() -> crate::Result<Vec<(&'static str, Convention)>> {
    use Convention::{Grouping, Number, Text};
    with_process_locale(|| {
        // SAFETY: the process-locale lock protects these libc-owned strings and
        // scalars. All fields are copied before querying or switching a locale.
        let (numeric, monetary, mon_grouping, signs, numbers) = unsafe {
            let lc = &*libc::localeconv();
            let monetary = [
                lc.int_curr_symbol,
                lc.currency_symbol,
                lc.mon_decimal_point,
                lc.mon_thousands_sep,
            ]
            .map(|ptr| CStr::from_ptr(ptr).to_owned());
            let signs =
                [lc.positive_sign, lc.negative_sign].map(|ptr| CStr::from_ptr(ptr).to_owned());
            let numbers = [
                ("int_frac_digits", lc.int_frac_digits),
                ("frac_digits", lc.frac_digits),
                ("p_cs_precedes", lc.p_cs_precedes),
                ("p_sep_by_space", lc.p_sep_by_space),
                ("n_cs_precedes", lc.n_cs_precedes),
                ("n_sep_by_space", lc.n_sep_by_space),
                ("p_sign_posn", lc.p_sign_posn),
                ("n_sign_posn", lc.n_sign_posn),
            ];
            (
                RawNumeric::copy(lc),
                monetary,
                copy_grouping(lc.mon_grouping),
                signs,
                numbers,
            )
        };
        let [international, currency, decimal, thousands] =
            category_text(libc::LC_MONETARY, monetary)?;
        // CPython decodes these signs after restoring the caller's character locale.
        let [positive, negative] = signs;
        let mut fields = vec![
            ("int_curr_symbol", Text(international)),
            ("currency_symbol", Text(currency)),
            ("mon_decimal_point", Text(decimal)),
            ("mon_thousands_sep", Text(thousands)),
            ("mon_grouping", Grouping(mon_grouping)),
            ("positive_sign", Text(decode_locale(&positive)?)),
            ("negative_sign", Text(decode_locale(&negative)?)),
        ];
        fields.extend(numbers.map(|(name, value)| (name, Number(value.into()))));
        let numeric = numeric.decode()?;
        fields.extend([
            ("decimal_point", Text(numeric.decimal_point)),
            ("thousands_sep", Text(numeric.thousands_sep)),
            ("grouping", Grouping(numeric.grouping)),
        ]);
        Ok(fields)
    })
}
