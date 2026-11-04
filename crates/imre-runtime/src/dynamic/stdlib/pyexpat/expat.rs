//! Owned streaming parser from the system's narrow-character Expat library.
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

type Raw = *mut c_void;
type Name = *const c_char;
type Start = unsafe extern "C" fn(Raw, Name, *const Name);
type OneName = unsafe extern "C" fn(Raw, Name);
type TwoNames = unsafe extern "C" fn(Raw, Name, Name);
type Text = unsafe extern "C" fn(Raw, Name, c_int);
type UnknownEncoding = unsafe extern "C" fn(Raw, Name, *mut Encoding) -> c_int;

/// The XML_Encoding layout from expat.h. A single-byte map has no owned data.
#[repr(C)]
pub(super) struct Encoding {
    map: [c_int; 256],
    data: Raw,
    convert: Option<unsafe extern "C" fn(Raw, Name) -> c_int>,
    release: Option<unsafe extern "C" fn(Raw)>,
}

impl Encoding {
    pub(super) fn single_byte(points: &[u32; 256]) -> Self {
        Self {
            map: points.map(|c| if c == 0xfffd { -1 } else { c as c_int }),
            data: ptr::null_mut(),
            convert: None,
            release: None,
        }
    }
}

#[derive(Default)]
pub(super) struct Handlers {
    pub(super) start_element: Option<Start>,
    pub(super) end_element: Option<OneName>,
    pub(super) text: Option<Text>,
    pub(super) start_ns: Option<TwoNames>,
    pub(super) end_ns: Option<OneName>,
    pub(super) comment: Option<OneName>,
    pub(super) pi: Option<TwoNames>,
    pub(super) default_expand: Option<Text>,
}

#[repr(C)]
struct Version {
    major: c_int,
    minor: c_int,
    micro: c_int,
}

#[repr(C)]
struct Feature {
    kind: c_int,
    name: Name,
    value: c_long,
}

// Each Expat symbol is declared together with its expat.h signature. On
// success the loader keeps its dlopen reference for the process lifetime.
macro_rules! functions {
    ($($field:ident = $name:literal : fn($($arg:ty),*) $(-> $ret:ty)?;)+) => {
        struct Api {
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)+
            defer: Option<unsafe extern "C" fn(Raw, u8) -> u8>,
        }

        impl Api {
            #[cfg(unix)]
            fn from_library(handle: NonNull<c_void>) -> Option<Self> {
                // SAFETY: handle is live and all symbol names are terminated.
                let address = |name: &CStr| NonNull::new(unsafe { libc::dlsym(handle.as_ptr(), name.as_ptr()) });
                Some(Self {
                    $($field: {
                        let p = address($name)?;
                        // SAFETY: each function pointer below has its symbol's C ABI.
                        unsafe { std::mem::transmute::<Raw, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(p.as_ptr()) }
                    },)+
                    defer: address(c"XML_SetReparseDeferralEnabled").map(|p| {
                        // SAFETY: optional since Expat 2.6; XML_Bool is unsigned char.
                        unsafe { std::mem::transmute::<Raw, unsafe extern "C" fn(Raw, u8) -> u8>(p.as_ptr()) }
                    }),
                })
            }
        }
    };
}

functions! {
    features = c"XML_GetFeatureList": fn() -> *const Feature;
    version = c"XML_ExpatVersionInfo": fn() -> Version;
    create = c"XML_ParserCreate": fn(Name) -> Raw;
    create_ns = c"XML_ParserCreateNS": fn(Name, c_char) -> Raw;
    free = c"XML_ParserFree": fn(Raw);
    parse = c"XML_Parse": fn(Raw, Name, c_int, c_int) -> c_int;
    stop = c"XML_StopParser": fn(Raw, u8) -> c_int;
    set_encoding = c"XML_SetEncoding": fn(Raw, Name) -> c_int;
    unknown_encoding = c"XML_SetUnknownEncodingHandler": fn(Raw, Option<UnknownEncoding>, Raw);
    user_data = c"XML_SetUserData": fn(Raw, Raw);
    elements = c"XML_SetElementHandler": fn(Raw, Option<Start>, Option<OneName>);
    set_ns_triplet = c"XML_SetReturnNSTriplet": fn(Raw, c_int);
    namespaces = c"XML_SetNamespaceDeclHandler": fn(Raw, Option<TwoNames>, Option<OneName>);
    text = c"XML_SetCharacterDataHandler": fn(Raw, Option<Text>);
    comment = c"XML_SetCommentHandler": fn(Raw, Option<OneName>);
    pi = c"XML_SetProcessingInstructionHandler": fn(Raw, Option<TwoNames>);
    default_expand = c"XML_SetDefaultHandlerExpand": fn(Raw, Option<Text>);
    code = c"XML_GetErrorCode": fn(Raw) -> c_int;
    line = c"XML_GetCurrentLineNumber": fn(Raw) -> u64;
    column = c"XML_GetCurrentColumnNumber": fn(Raw) -> u64;
    byte_index = c"XML_GetCurrentByteIndex": fn(Raw) -> i64;
    specified = c"XML_GetSpecifiedAttributeCount": fn(Raw) -> c_int;
    message = c"XML_ErrorString": fn(c_int) -> Name;
}

impl Api {
    fn load() -> Option<Self> {
        // XML_Size and XML_Index use C long or long long depending on XML_LARGE_SIZE.
        // Both have the 64-bit ABI declared above on the LP64 targets we load.
        #[cfg(all(
            target_pointer_width = "64",
            any(target_os = "linux", target_os = "macos")
        ))]
        {
            #[cfg(target_os = "linux")]
            let name = c"libexpat.so.1";
            #[cfg(target_os = "macos")]
            let name = c"libexpat.1.dylib";
            // SAFETY: the terminated soname is resolved by the platform loader.
            let handle = NonNull::new(unsafe {
                libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
            })?;
            let api = Self::from_library(handle).filter(Self::supports_narrow_namespaces);
            if api.is_none() {
                // SAFETY: no pointers from this handle have been published.
                unsafe { libc::dlclose(handle.as_ptr()) };
            }
            api
        }
        #[cfg(not(all(
            target_pointer_width = "64",
            any(target_os = "linux", target_os = "macos")
        )))]
        None
    }

    #[cfg(unix)]
    fn supports_narrow_namespaces(&self) -> bool {
        // SAFETY: XML_GetFeatureList returns an immutable XML_FEATURE_END-terminated
        // array. Reading the enum avoids dereferencing potentially wide names.
        unsafe {
            let mut p = (self.features)();
            let (mut chars, mut literals, mut namespaces) = (false, false, false);
            while !p.is_null() && (*p).kind != 0 {
                match (*p).kind {
                    6 => chars = (*p).value == 1,    // XML_FEATURE_SIZEOF_XML_CHAR
                    7 => literals = (*p).value == 1, // XML_FEATURE_SIZEOF_XML_LCHAR
                    8 => namespaces = true,          // XML_FEATURE_NS
                    _ => {}
                }
                p = p.add(1);
            }
            chars && literals && namespaces
        }
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref()
}

pub(super) fn version() -> Option<(i32, i32, i32)> {
    // SAFETY: the resolved library remains loaded for the process lifetime.
    let v = unsafe { (api()?.version)() };
    Some((v.major, v.minor, v.micro))
}

pub(super) fn error_message(code: i32) -> Option<&'static str> {
    // SAFETY: a recognized code returns a static terminated error message in
    // the narrow-character library; an unknown code returns null.
    unsafe {
        let s = (api()?.message)(code);
        if s.is_null() {
            None
        } else {
            CStr::from_ptr(s).to_str().ok()
        }
    }
}

pub(super) enum CreateError {
    Unavailable,
    NoMemory,
}

pub(super) struct ParseError {
    pub(super) code: i32,
    pub(super) line: u64,
    pub(super) column: u64,
}

pub(super) struct Parser {
    api: &'static Api,
    ptr: NonNull<c_void>,
    parsing: Cell<bool>,
}

impl Parser {
    pub(super) fn new(
        encoding: Option<&CStr>,
        separator: Option<c_char>,
    ) -> Result<Self, CreateError> {
        let api = api().ok_or(CreateError::Unavailable)?;
        let encoding = encoding.map_or(ptr::null(), CStr::as_ptr);
        // SAFETY: Expat duplicates the optional encoding string before returning.
        let raw = unsafe {
            match separator {
                Some(sep) => (api.create_ns)(encoding, sep),
                None => (api.create)(encoding),
            }
        };
        Ok(Self {
            api,
            ptr: NonNull::new(raw).ok_or(CreateError::NoMemory)?,
            parsing: Cell::new(false),
        })
    }

    /// Update listeners synchronously, including changes made inside a callback.
    ///
    /// # Safety
    /// All callbacks must accept the user pointer passed to the active parse,
    /// and none may unwind across the C ABI.
    pub(super) unsafe fn set_handlers(&self, handlers: &Handlers) {
        let p = self.ptr.as_ptr();
        // SAFETY: p is owned by self and the callback types match expat.h.
        unsafe {
            (self.api.elements)(p, handlers.start_element, handlers.end_element);
            (self.api.namespaces)(p, handlers.start_ns, handlers.end_ns);
            (self.api.text)(p, handlers.text);
            (self.api.comment)(p, handlers.comment);
            (self.api.pi)(p, handlers.pi);
            (self.api.default_expand)(p, handlers.default_expand);
        }
    }

    /// # Safety
    /// The given callbacks must accept `user` throughout this call and must
    /// never unwind across C frames. Expat invokes them inline before returning.
    pub(super) unsafe fn parse(
        &self,
        bytes: &[u8],
        final_chunk: bool,
        user: Raw,
        handlers: &Handlers,
        unknown_encoding: UnknownEncoding,
    ) -> Result<(), ParseError> {
        if self.parsing.replace(true) {
            return Err(ParseError {
                code: 23,
                line: self.line(),
                column: self.column(),
            }); // XML_ERROR_UNEXPECTED_STATE
        }
        let p = self.ptr.as_ptr();
        // SAFETY: the owner lives throughout this call. The callback context
        // remains valid until XML_Parse returns and is cleared afterwards.
        let status = unsafe {
            (self.api.user_data)(p, user);
            (self.api.unknown_encoding)(p, Some(unknown_encoding), user);
            self.set_handlers(handlers);
            // Bound each C int length, matching CPython's feed chunk size. Even
            // an empty final call goes through XML_Parse to validate end-of-input.
            const CHUNK: usize = 1 << 20;
            let mut input = bytes;
            let status = loop {
                let n = input.len().min(CHUNK);
                let last = n == input.len();
                let rc = (self.api.parse)(
                    p,
                    input.as_ptr().cast(),
                    n as c_int,
                    (last && final_chunk).into(),
                );
                if rc != 1 || last {
                    break rc;
                }
                input = &input[n..];
            };
            (self.api.unknown_encoding)(p, None, ptr::null_mut());
            (self.api.user_data)(p, ptr::null_mut());
            status
        };
        self.parsing.set(false);
        if status == 1 {
            Ok(())
        } else {
            Err(ParseError {
                code: self.error_code(),
                line: self.line(),
                column: self.column(),
            })
        }
    }

    /// Terminate the parser after a handler raises, including during the
    /// character-buffer flush immediately following a successful XML_Parse.
    pub(super) fn stop(&self) {
        // SAFETY: self owns the live parser. A non-resumable stop ends a
        // nonfinal parse; stopping an already finished parser reports FINISHED.
        unsafe { (self.api.stop)(self.ptr.as_ptr(), 0) };
    }

    pub(super) fn set_utf8(&self) -> bool {
        // SAFETY: Expat owns its copy of this terminated encoding name.
        unsafe { (self.api.set_encoding)(self.ptr.as_ptr(), c"UTF-8".as_ptr()) != 0 }
    }

    pub(super) fn set_namespace_prefixes(&self, enabled: bool) {
        // SAFETY: the parser is live and Expat does not retain any pointers.
        unsafe { (self.api.set_ns_triplet)(self.ptr.as_ptr(), enabled.into()) }
    }

    pub(super) fn supports_reparse_deferral(&self) -> bool {
        self.api.defer.is_some()
    }

    pub(super) fn set_reparse_deferral(&self, enabled: bool) -> bool {
        self.api.defer.is_some_and(|set| {
            // SAFETY: self owns the parser and the flag is a valid XML_Bool.
            unsafe { set(self.ptr.as_ptr(), enabled.into()) != 0 }
        })
    }

    pub(super) fn line(&self) -> u64 {
        // SAFETY: self owns the parser, including while its callback observes the position.
        unsafe { (self.api.line)(self.ptr.as_ptr()) }
    }

    pub(super) fn column(&self) -> u64 {
        // SAFETY: self owns the parser.
        unsafe { (self.api.column)(self.ptr.as_ptr()) }
    }

    pub(super) fn byte_index(&self) -> i64 {
        // SAFETY: self owns the parser.
        unsafe { (self.api.byte_index)(self.ptr.as_ptr()) }
    }

    pub(super) fn error_code(&self) -> i32 {
        // SAFETY: self owns the parser.
        unsafe { (self.api.code)(self.ptr.as_ptr()) }
    }

    pub(super) fn specified_attribute_count(&self) -> usize {
        // SAFETY: self owns the parser; the callback queries this during a start event.
        unsafe { (self.api.specified)(self.ptr.as_ptr()).max(0) as usize }
    }
}

impl Drop for Parser {
    fn drop(&mut self) {
        // SAFETY: the owned parser is freed once, after the last Rust owner is gone.
        unsafe { (self.api.free)(self.ptr.as_ptr()) };
    }
}
