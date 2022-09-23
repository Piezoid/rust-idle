// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.
use std::fmt;
use std::io;
use std::result;

type CowStr = std::borrow::Cow<'static, str>;

/// A anyhow error type on a diet.
/// Sepcialized for io::Error and optional &'static str context.
pub struct Error(Box<ErrorRepr>);

pub type Result<T> = result::Result<T, Error>;

struct ErrorRepr {
    chain: Vec<CowStr>,
    source: Option<io::Error>,
}

impl fmt::Display for Error {
    #[cold]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let repr = &*self.0;
        for ctx in repr
            .chain
            .iter()
            .skip(usize::from(repr.source.is_none()))
            .rev()
        {
            f.write_str(ctx)?;
            f.write_str(": ")?;
        }
        if let Some(source) = &repr.source {
            fmt::Display::fmt(source, f)
        } else if let Some(ctx) = repr.chain.first() {
            f.write_str(ctx)
        } else {
            result::Result::Ok(())
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {
    #[inline(always)]
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source.as_ref().map(|e| e as _)
    }
}

impl From<CowStr> for Error {
    #[cold]
    fn from(msg: CowStr) -> Self {
        Error(Box::new(ErrorRepr {
            chain: vec![msg],
            source: None,
        }))
    }
}

impl From<&'static str> for Error {
    fn from(msg: &'static str) -> Self {
        Self::from(CowStr::Borrowed(msg))
    }
}

impl From<String> for Error {
    fn from(msg: String) -> Self {
        Self::from(CowStr::Owned(msg))
    }
}

impl From<io::Error> for Error {
    #[cold]
    fn from(source: io::Error) -> Self {
        Error(Box::new(ErrorRepr {
            chain: Vec::new(),
            source: Some(source),
        }))
    }
}

impl From<i32> for Error {
    fn from(errno: i32) -> Self {
        Self::from(io::Error::from_raw_os_error(errno))
    }
}

pub trait Context<T> {
    fn context<M>(self, msg: M) -> Result<T>
    where
        CowStr: From<M>;
    fn with_context<M, F>(self, f: F) -> Result<T>
    where
        CowStr: From<M>,
        F: FnOnce() -> M;
}

impl<T, E> Context<T> for result::Result<T, E>
where
    Error: From<E>,
{
    #[inline(always)]
    fn context<M>(self, msg: M) -> Result<T>
    where
        CowStr: From<M>,
    {
        self.map_err(
            #[cold]
            |e| add_context(e, msg),
        )
    }

    #[inline(always)]
    fn with_context<M, F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> M,
        CowStr: From<M>,
    {
        self.map_err(
            #[cold]
            |e| add_context(e, f()),
        )
    }
}

#[cold]
fn add_context<E, M>(error: E, msg: M) -> Error
where
    Error: From<E>,
    CowStr: From<M>,
{
    let mut this = Error::from(error);
    this.0.chain.push(msg.into());
    this
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_sizeofs() {
        assert_eq!(
            std::mem::size_of::<Result<usize>>(),
            2 * std::mem::size_of::<usize>()
        );
    }

    fn assert_result_display(res: Result<()>, expected: &str) {
        assert_eq!(res.err().unwrap().to_string(), expected);
    }

    #[test]
    fn it_works() {
        assert_result_display(Err(0_i32.into()), "Success (os error 0)");
        assert_result_display(Err("a".into()), "a");
        assert_result_display(Err("c").context("b").with_context(|| "a"), "a: b: c");
        assert_result_display(
            Err(0_i32).context("b").with_context(|| "a"),
            "a: b: Success (os error 0)",
        );
    }
}
