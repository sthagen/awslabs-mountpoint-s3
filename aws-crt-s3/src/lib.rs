#![deny(missing_debug_implementations, missing_docs)]

//! Rust bindings for the AWS Common Runtime.

use aws_crt_s3_sys::*;

pub mod auth;
pub mod common;
pub mod http;
pub mod io;
pub mod s3;

use std::ptr::NonNull;
use std::{ffi::OsStr, os::unix::prelude::OsStrExt};

use crate::common::error::Error;

pub(crate) mod private {
    /// Seals a trait to prevent clients from implementing it for their own types, since this trait
    /// is only accessible to this crate.
    pub trait Sealed {}
}

pub(crate) trait StringExt {
    unsafe fn as_aws_byte_cursor(&self) -> aws_byte_cursor;
}

impl<S: AsRef<OsStr>> StringExt for S {
    /// Safety: the user *must not* mutate the bytes pointed at by this cursor
    /// Also, the user must be careful that the aws_byte_cursor does not outlive self.
    unsafe fn as_aws_byte_cursor(&self) -> aws_byte_cursor {
        aws_byte_cursor {
            ptr: self.as_ref().as_bytes().as_ptr() as *mut _,
            len: self.as_ref().as_bytes().len(),
        }
    }
}

/// Translate the common "return a null pointer on failure" pattern into Results that pull the last
/// error from the CRT.
pub(crate) trait CrtError: Sized {
    type Return;

    /// Safety: This must only be used immediately on a pointer returned from the CRT, with no other
    /// CRT code being run beforehand, or else it will return the wrong error.
    unsafe fn ok_or_last_error(self) -> Result<Self::Return, Error>;
}

impl<T> CrtError for *const T {
    type Return = NonNull<T>;

    unsafe fn ok_or_last_error(self) -> Result<Self::Return, Error> {
        NonNull::new(self as *mut T).ok_or_else(|| Error::last_error())
    }
}

impl<T> CrtError for *mut T {
    type Return = NonNull<T>;

    unsafe fn ok_or_last_error(self) -> Result<Self::Return, Error> {
        NonNull::new(self as *mut T).ok_or_else(|| Error::last_error())
    }
}

/// Some CRT functions return an int that is either AWS_OP_SUCCESS or AWS_OP_ERR, and the caller
/// should use last_error to find out what happened. This simplifies that pattern.
impl CrtError for i32 {
    type Return = ();

    unsafe fn ok_or_last_error(self) -> Result<Self::Return, Error> {
        match self {
            AWS_OP_SUCCESS => Ok(()),
            AWS_OP_ERR => Err(Error::last_error()),
            // This case shouldn't happen if used correctly since we should use this on functions
            // that only return SUCCESS or ERR. But if it does happen, we can attempt to convert the
            // error code directly, which may or may not work (but at least the Error won't be swallowed).
            n => Err(common::error::Error::from(n)),
        }
    }
}

/// Workaround until Result::inspect_err is stable.
pub(crate) trait ResultExt: Sized {
    fn on_err<F>(self, f: F) -> Self
    where
        F: FnOnce();
}

impl<T, E> ResultExt for Result<T, E> {
    fn on_err<F>(self, f: F) -> Result<T, E>
    where
        F: FnOnce(),
    {
        match self {
            Ok(val) => Ok(val),
            Err(err) => {
                f();
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::common::rust_log_adapter::RustLogAdapter;

    /// Enable tracing when running unit tests.
    #[ctor::ctor]
    fn init_tracing_subscriber() {
        RustLogAdapter::try_init().expect("unable to install CRT log adapter");
        tracing_subscriber::fmt::init();
    }
}