// Copyright 2023, The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Routines for parsing bootargs

#[cfg(not(test))]
use alloc::format;
#[cfg(not(test))]
use alloc::string::String;
use core::ffi::CStr;

/// A single boot argument ex: "panic", "init=", or "foo=1,2,3".
pub struct BootArg<'a> {
    arg: &'a str,
    equal_sign: Option<usize>,
}

impl AsRef<str> for BootArg<'_> {
    fn as_ref(&self) -> &str {
        self.arg
    }
}

impl BootArg<'_> {
    /// Name of the boot argument
    pub fn name(&self) -> &str {
        if let Some(n) = self.equal_sign {
            &self.arg[..n]
        } else {
            self.arg
        }
    }

    /// Optional value of the boot aragument. This includes the '=' prefix.
    pub fn value(&self) -> Option<&str> {
        Some(&self.arg[self.equal_sign?..])
    }
}

/// Iterator that iteratos over bootargs
pub struct BootArgsIterator<'a> {
    arg: &'a str,
}

impl<'a> BootArgsIterator<'a> {
    /// Creates a new iterator from the raw boot args. The input has to be encoded in ASCII
    pub fn new(bootargs: &'a CStr) -> Result<Self, String> {
        let arg = bootargs.to_str().map_err(|e| format!("{e}"))?;
        if !arg.is_ascii() {
            return Err(format!("{arg:?} is not ASCII"));
        }

        Ok(Self { arg })
    }

    // Finds the end of a value in the given string `s`, and returns the index of the end. A value
    // can have spaces if quoted. The quote character can't be escaped.
    fn find_value_end(s: &str) -> usize {
        let mut in_quote = false;
        for (i, c) in s.char_indices() {
            if c == '"' {
                in_quote = !in_quote;
            } else if c.is_whitespace() && !in_quote {
                return i;
            }
        }
        s.len()
    }
}

impl<'a> Iterator for BootArgsIterator<'a> {
    type Item = BootArg<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Skip spaces to find the start of a name. If there's nothing left, that's the end of the
        // iterator.
        let arg = self.arg.trim_start();
        self.arg = arg; // advance before returning
        if arg.is_empty() {
            return None;
        }
        // Name ends with either whitespace or =. If it ends with =, the value comes immediately
        // after.
        let name_end = arg.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(arg.len());
        let (arg, equal_sign) = match arg.chars().nth(name_end) {
            Some('=') => {
                let value_end = name_end + Self::find_value_end(&arg[name_end..]);
                (&arg[..value_end], Some(name_end))
            }
            _ => (&arg[..name_end], None),
        };
        self.arg = &self.arg[arg.len()..]; // advance before returning
        Some(BootArg { arg, equal_sign })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cstr::cstr;

    fn check(raw: &CStr, expected: Result<&[(&str, Option<&str>)], ()>) {
        let actual = BootArgsIterator::new(raw);
        assert_eq!(actual.is_err(), expected.is_err(), "Unexpected result with {raw:?}");
        if actual.is_err() {
            return;
        }
        let mut actual = actual.unwrap();

        for (name, value) in expected.unwrap() {
            let actual = actual.next();
            assert!(actual.is_some(), "Expected ({}, {:?}) from {raw:?}", name, value);
            let actual = actual.unwrap();
            assert_eq!(name, &actual.name(), "Unexpected name from {raw:?}");
            assert_eq!(value, &actual.value(), "Unexpected value from {raw:?}");
        }
        let remaining = actual.next();
        assert!(
            remaining.is_none(),
            "Unexpected extra item from {raw:?}. Got ({}, {:?})",
            remaining.as_ref().unwrap().name(),
            remaining.as_ref().unwrap().value()
        );
    }

    #[test]
    fn empty() {
        check(cstr!(""), Ok(&[]));
        check(cstr!("    "), Ok(&[]));
        check(cstr!("  \n  "), Ok(&[]));
    }

    #[test]
    fn single() {
        check(cstr!("foo"), Ok(&[("foo", None)]));
        check(cstr!("   foo"), Ok(&[("foo", None)]));
        check(cstr!("foo   "), Ok(&[("foo", None)]));
        check(cstr!("   foo   "), Ok(&[("foo", None)]));
    }

    #[test]
    fn single_with_value() {
        check(cstr!("foo=bar"), Ok(&[("foo", Some("=bar"))]));
        check(cstr!("   foo=bar"), Ok(&[("foo", Some("=bar"))]));
        check(cstr!("foo=bar   "), Ok(&[("foo", Some("=bar"))]));
        check(cstr!("   foo=bar   "), Ok(&[("foo", Some("=bar"))]));

        check(cstr!("foo="), Ok(&[("foo", Some("="))]));
        check(cstr!("   foo="), Ok(&[("foo", Some("="))]));
        check(cstr!("foo=   "), Ok(&[("foo", Some("="))]));
        check(cstr!("   foo=   "), Ok(&[("foo", Some("="))]));
    }

    #[test]
    fn single_with_quote() {
        check(cstr!("foo=hello\" \"world"), Ok(&[("foo", Some("=hello\" \"world"))]));
    }

    #[test]
    fn invalid_encoding() {
        check(CStr::from_bytes_with_nul(&[255, 255, 255, 0]).unwrap(), Err(()));
    }

    #[test]
    fn multiple() {
        check(
            cstr!(" a=b   c=d   e=  f g  "),
            Ok(&[("a", Some("=b")), ("c", Some("=d")), ("e", Some("=")), ("f", None), ("g", None)]),
        );
        check(
            cstr!("   a=b  \n c=d      e=  f g"),
            Ok(&[("a", Some("=b")), ("c", Some("=d")), ("e", Some("=")), ("f", None), ("g", None)]),
        );
    }

    #[test]
    fn incomplete_quote() {
        check(
            cstr!("foo=incomplete\" quote bar=y"),
            Ok(&[("foo", Some("=incomplete\" quote bar=y"))]),
        );
    }

    #[test]
    fn complex() {
        check(cstr!("  a  a1=  b=c d=e,f,g x=\"value with quote\" y=val\"ue with \"multiple\" quo\"te  "), Ok(&[
            ("a", None),
            ("a1", Some("=")),
            ("b", Some("=c")),
            ("d", Some("=e,f,g")),
            ("x", Some("=\"value with quote\"")),
            ("y", Some("=val\"ue with \"multiple\" quo\"te")),
        ]));
    }
}
