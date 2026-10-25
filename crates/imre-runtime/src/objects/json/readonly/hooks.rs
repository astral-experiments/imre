//! Native numeric callbacks receive each original token before the scanner advances.
use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;

use super::{Node, deep, number};
use crate::objects::exceptions::imre_error_pending;
use crate::objects::json::{binary, syntax};
use crate::objects::{Object, acquire_rc};
use crate::{Bytes, Error, Int, Str};

/// The compiler roots the bound receiver and verifies each callback's signature.
#[repr(C)]
pub struct Callback {
    code: *const c_void,
    context: *const c_void,
}

#[derive(Clone, Copy)]
struct Hook {
    mode: u8,
    callback: *const Callback,
}

impl Hook {
    /// A missing node means the called function already raised an exception.
    unsafe fn apply(self, raw: &str) -> Option<Node> {
        if self.mode == 0 {
            return Some(number(raw));
        }
        let token = match Str::try_new(raw) {
            Ok(value) => Rc::new(value),
            Err(_) => return Error::allocation().returning(None),
        };
        let input = Rc::as_ptr(&token);
        fn checked<T>(value: T) -> Option<T> {
            (imre_error_pending() == 0).then_some(value)
        }
        // SAFETY: the token and receiver stay live until return. Only adopt a
        // callback's result after checking its pending-exception sentinel.
        unsafe {
            match self.mode {
                1 => checked(crate::floats::imre_float_parse(input)).map(Node::Float),
                2 => Some(Node::String(token)),
                3 => Some(Node::String(Rc::new(Str::from(raw.to_ascii_uppercase())))),
                4 => checked(crate::imre_int_parse(input)).map(|v| Node::Integer(Int(v))),
                5 => Some(Node::String(Rc::new(Str::from(raw.to_ascii_lowercase())))),
                6..=10 => {
                    let callback = &*self.callback;
                    macro_rules! call {
                        ($result:ty) => {{
                            let f: unsafe extern "C" fn(*const c_void, *const Str) -> $result =
                                std::mem::transmute(callback.code);
                            checked(f(callback.context, input))?
                        }};
                    }
                    Some(match self.mode {
                        6 => Node::Integer(Int(call!(usize))),
                        7 => Node::Float(call!(f64)),
                        8 => Node::String(Rc::from_raw(call!(*const Str))),
                        9 => Node::Bool(call!(bool)),
                        _ => {
                            call!(u8);
                            Node::Null
                        }
                    })
                }
                _ => unreachable!("checked JSON number hook mode"),
            }
        }
    }
}

unsafe fn parse(
    text: &str,
    owner: impl FnOnce() -> Option<Rc<Str>>,
    strict: bool,
    hooks: [Hook; 3],
    reject_bom: bool,
) -> *const Object {
    if reject_bom && text.starts_with('\u{feff}') {
        let error = owner().map_or_else(Error::allocation, |doc| {
            syntax::at_error(doc, 0, "Unexpected UTF-8 BOM (decode using utf-8-sig)")
        });
        return error.returning(ptr::null());
    }
    deep::loads_with_numbers(text, strict, owner, |raw| {
        let index = if matches!(raw, "Infinity" | "-Infinity" | "NaN") {
            2
        } else if raw.contains(['.', 'e', 'E']) {
            1
        } else {
            0
        };
        // SAFETY: the caller holds all three verified callbacks throughout the scan.
        unsafe { hooks[index].apply(raw) }
    })
}

/// # Safety
/// The compiler supplies a live Unicode or bytes input, roots the callback
/// records, and packs the three checked hook modes in bytes, integer first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_loads_hooked(
    input: *const c_void,
    is_binary: u32,
    strict: u32,
    modes: u32,
    int: *const Callback,
    float: *const Callback,
    constant: *const Callback,
) -> *const Object {
    let callbacks = [int, float, constant];
    let hooks = std::array::from_fn(|i| Hook {
        mode: (modes >> (8 * i)) as u8,
        callback: callbacks[i],
    });
    // SAFETY: input and all callback owners outlive this complete parse.
    unsafe {
        if strict != 0 && modes == 0 {
            return if is_binary == 0 {
                super::imre_json_loads(input.cast())
            } else {
                binary::imre_json_loads_bytes(input.cast())
            };
        }
        if is_binary != 0 {
            let bytes = acquire_rc::<Bytes>(input as usize);
            match binary::decode(&bytes) {
                Ok(text) => parse(
                    &text,
                    || Str::try_new(text.as_ref()).ok().map(Rc::new),
                    strict != 0,
                    hooks,
                    false,
                ),
                Err(error) => error.returning(ptr::null()),
            }
        } else {
            parse(
                &*input.cast::<Str>(),
                || Some(acquire_rc(input as usize)),
                strict != 0,
                hooks,
                true,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_binary_with_a_hook_retains_the_original_bytes() {
        let bytes = Rc::new(vec![0x80]);
        // SAFETY: mode 2 is the built-in str conversion; no callable pointer is needed.
        let result = unsafe {
            imre_json_loads_hooked(
                Rc::as_ptr(&bytes).cast(),
                1,
                1,
                2,
                ptr::null(),
                ptr::null(),
                ptr::null(),
            )
        };
        assert!(result.is_null());
        let error = crate::objects::exceptions::take_pending().unwrap();
        assert_eq!(Rc::strong_count(&bytes), 2);
        drop(error);
        assert_eq!(Rc::strong_count(&bytes), 1);
    }
}
