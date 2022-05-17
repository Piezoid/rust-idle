// Copyright (c) 2022 Maël Kerbiriou <m431.kerbiriou@gmail.com>. All rights
// reserved. Use of this source is governed by MIT License that can be found in
// the LICENSE file.

use std::fmt;
use std::io;
use std::result;

type CowStr = std::borrow::Cow<'static, str>;

/// A anyhow error type on a diet
pub struct Error {
    chain: Vec<CowStr>,
    source: Option<io::Error>,
}

pub type Result<T> = result::Result<T, Error>;

#[allow(non_snake_case)]
pub fn Ok<T>(v: T) -> Result<T> {
    Result::Ok(v)
}

impl Error {
    fn push<T>(mut self, msg: T) -> Self
    where
        CowStr: From<T>,
    {
        self.chain.push(msg.into());
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for ctx in self.chain.iter().skip(self.source.is_none() as usize).rev() {
            f.write_str(ctx)?;
            f.write_str(": ")?;
        }
        if let Some(source) = &self.source {
            fmt::Display::fmt(source, f)
        } else if let Some(ctx) = self.chain.first() {
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
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e as _)
    }
}

impl From<CowStr> for Error {
    fn from(msg: CowStr) -> Self {
        Self {
            chain: vec![msg.into()],
            source: None,
        }
    }
}

impl From<&'static str> for Error {
    fn from(msg: &'static str) -> Self {
        Error::from(CowStr::from(msg))
    }
}

impl From<String> for Error {
    fn from(msg: String) -> Self {
        Error::from(CowStr::from(msg))
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self {
            chain: vec![],
            source: Some(source),
        }
    }
}

impl From<i32> for Error {
    fn from(errno: i32) -> Self {
        Error::from(io::Error::from_raw_os_error(errno))
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
    fn context<M>(self, msg: M) -> Result<T>
    where
        CowStr: From<M>,
    {
        self.map_err(|e| Error::from(e).push(msg))
    }

    fn with_context<M, F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> M,
        CowStr: From<M>,
    {
        self.map_err(|e| Error::from(e).push(f()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_result_display(res: Result<()>, expected: &str) {
        assert_eq!(res.err().unwrap().to_string(), expected);
    }

    #[test]
    fn it_works() {
        assert_result_display(Err((0 as i32).into()), "Success (os error 0)");
        assert_result_display(Err("a".into()), "a");
        assert_result_display(Err("c").context("b").with_context(|| "a"), "a: b: c");
        assert_result_display(
            Err(0 as i32).context("b").with_context(|| "a"),
            "a: b: Success (os error 0)",
        );
    }
}
