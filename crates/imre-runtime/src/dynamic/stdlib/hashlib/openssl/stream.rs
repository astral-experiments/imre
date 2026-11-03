//! Fixed-size OpenSSL digest contexts. Reading a digest always finalizes a copy.
use std::ffi::{c_int, c_uint, c_void};
use std::ptr::{self, NonNull};

use super::{Digest, Error, Provider, Result};

type CtxNew = unsafe extern "C" fn() -> *mut c_void;
type CtxFree = unsafe extern "C" fn(*mut c_void);
type Init = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int;
type Update = unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> c_int;
type Copy = unsafe extern "C" fn(*mut c_void, *const c_void) -> c_int;
type Final = unsafe extern "C" fn(*mut c_void, *mut u8, *mut c_uint) -> c_int;
type BlockSize = unsafe extern "C" fn(*const c_void) -> c_int;

pub(super) struct Functions {
    new: CtxNew,
    free: CtxFree,
    init: Init,
    update: Update,
    copy: Copy,
    finalize: Final,
    block_size: BlockSize,
}

impl Functions {
    #[cfg(unix)]
    pub(super) fn load(handle: NonNull<c_void>) -> Option<Self> {
        Some(Self {
            new: symbol!(handle, c"EVP_MD_CTX_new", CtxNew)?,
            free: symbol!(handle, c"EVP_MD_CTX_free", CtxFree)?,
            init: symbol!(handle, c"EVP_DigestInit_ex", Init)?,
            update: symbol!(handle, c"EVP_DigestUpdate", Update)?,
            copy: symbol!(handle, c"EVP_MD_CTX_copy_ex", Copy)?,
            finalize: symbol!(handle, c"EVP_DigestFinal_ex", Final)?,
            block_size: symbol!(handle, c"EVP_MD_get_block_size", BlockSize)?,
        })
    }
}

pub(crate) struct Stream {
    provider: &'static Provider,
    ctx: NonNull<c_void>,
    pub(in crate::dynamic::stdlib::hashlib) name: &'static str,
    pub(in crate::dynamic::stdlib::hashlib) digest_size: usize,
    pub(in crate::dynamic::stdlib::hashlib) block_size: usize,
}

// SAFETY: EVP_MD_CTX has exclusive ownership. The boxed hash shares it only behind a mutex.
unsafe impl Send for Stream {}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner, and the provider remains loaded.
        unsafe { (self.provider.stream.free)(self.ctx.as_ptr()) };
    }
}

impl Stream {
    pub(in crate::dynamic::stdlib::hashlib) fn new(
        name: &'static str,
        digest: &Digest,
    ) -> Result<Self> {
        let size = digest.size();
        // SAFETY: the fetched EVP_MD is live for this call.
        let block = unsafe { (digest.provider.stream.block_size)(digest.md.as_ptr()) };
        if !(1..=64).contains(&size) || block < 1 {
            return Err(Error::new("ValueError", "invalid fixed digest parameters"));
        }
        let stream = Self::empty(digest.provider, name, size as usize, block as usize)?;
        // SAFETY: initialization retains the fetched digest in the context. No custom ENGINE is used.
        if unsafe {
            (stream.provider.stream.init)(stream.ctx.as_ptr(), digest.md.as_ptr(), ptr::null_mut())
        } != 1
        {
            return Err(stream.provider.error("digest initialization failed"));
        }
        Ok(stream)
    }

    fn empty(
        provider: &'static Provider,
        name: &'static str,
        digest_size: usize,
        block_size: usize,
    ) -> Result<Self> {
        // SAFETY: allocates an independent EVP_MD_CTX, or returns null on failure.
        let ctx = NonNull::new(unsafe { (provider.stream.new)() }).ok_or_else(|| {
            Error::new(
                "MemoryError",
                "could not allocate an OpenSSL digest context",
            )
        })?;
        Ok(Self {
            provider,
            ctx,
            name,
            digest_size,
            block_size,
        })
    }

    pub(in crate::dynamic::stdlib::hashlib) fn update(&mut self, data: &[u8]) -> Result<()> {
        // SAFETY: the exclusive context and input slice are live for the synchronous update.
        if unsafe {
            (self.provider.stream.update)(self.ctx.as_ptr(), data.as_ptr().cast(), data.len())
        } != 1
        {
            return Err(self.provider.error("digest update failed"));
        }
        Ok(())
    }

    pub(in crate::dynamic::stdlib::hashlib) fn try_copy(&self) -> Result<Self> {
        let copy = Self::empty(self.provider, self.name, self.digest_size, self.block_size)?;
        // SAFETY: the destination is empty and distinct from the initialized source.
        if unsafe { (self.provider.stream.copy)(copy.ctx.as_ptr(), self.ctx.as_ptr()) } != 1 {
            return Err(self.provider.error("digest copy failed"));
        }
        Ok(copy)
    }

    pub(in crate::dynamic::stdlib::hashlib) fn digest(&self) -> Result<Vec<u8>> {
        let copy = self.try_copy()?;
        let mut bytes = vec![0; self.digest_size];
        let mut written = 0;
        // SAFETY: this buffer has the fixed EVP_MD_get_size length of the copied context.
        if unsafe {
            (self.provider.stream.finalize)(copy.ctx.as_ptr(), bytes.as_mut_ptr(), &mut written)
        } != 1
        {
            return Err(self.provider.error("digest finalization failed"));
        }
        if written as usize != bytes.len() {
            return Err(Error::new("ValueError", "unexpected OpenSSL digest length"));
        }
        Ok(bytes)
    }
}
