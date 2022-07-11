//! Module for the [`XmlSource`] trait.

use std::io::{self, BufRead};

use crate::{Error, Result};

use super::{is_whitespace, BangType, ReadElementState};

/// Represents an input for a reader that can return borrowed data.
///
/// There are two implementors of this trait: generic one that read data from
/// `Self`, copies some part of it into a provided buffer of type `B` and then
/// returns data that borrow from that buffer.
///
/// The other implementor is for `&[u8]` and instead of copying data returns
/// borrowed data from `Self` instead. This implementation allows zero-copy
/// deserialization.
///
/// # Parameters
/// - `'r`: lifetime of a buffer from which events will borrow
/// - `B`: a type of a buffer that can be used to store data read from `Self` and
///   from which events can borrow
pub(super) trait XmlSource<'r, B> {
    /// Read input until `byte` is found or end of input is reached.
    ///
    /// Returns a slice of data read up to `byte`, which does not include into result.
    /// If input (`Self`) is exhausted, returns `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut position = 0;
    /// let mut input = b"abc*def".as_ref();
    /// //                    ^= 4
    ///
    /// assert_eq!(
    ///     input.read_bytes_until(b'*', (), &mut position).unwrap(),
    ///     Some(b"abc".as_ref())
    /// );
    /// assert_eq!(position, 4); // position after the symbol matched
    /// ```
    ///
    /// # Parameters
    /// - `byte`: Byte for search
    /// - `buf`: Buffer that could be filled from an input (`Self`) and
    ///   from which [events] could borrow their data
    /// - `position`: Will be increased by amount of bytes consumed
    ///
    /// [events]: crate::events::Event
    fn read_bytes_until(
        &mut self,
        byte: u8,
        buf: B,
        position: &mut usize,
    ) -> Result<Option<&'r [u8]>>;

    /// Read input until comment, CDATA or processing instruction is finished.
    ///
    /// This method expect that `<` already was read.
    ///
    /// Returns a slice of data read up to end of comment, CDATA or processing
    /// instruction (`>`), which does not include into result.
    ///
    /// If input (`Self`) is exhausted and nothing was read, returns `None`.
    ///
    /// # Parameters
    /// - `buf`: Buffer that could be filled from an input (`Self`) and
    ///   from which [events] could borrow their data
    /// - `position`: Will be increased by amount of bytes consumed
    ///
    /// [events]: crate::events::Event
    fn read_bang_element(
        &mut self,
        buf: B,
        position: &mut usize,
    ) -> Result<Option<(BangType, &'r [u8])>>;

    /// Read input until XML element is closed by approaching a `>` symbol.
    /// Returns `Some(buffer)` that contains a data between `<` and `>` or
    /// `None` if end-of-input was reached and nothing was read.
    ///
    /// Derived from `read_until`, but modified to handle XML attributes
    /// using a minimal state machine.
    ///
    /// Attribute values are [defined] as follows:
    /// ```plain
    /// AttValue := '"' (([^<&"]) | Reference)* '"'
    ///           | "'" (([^<&']) | Reference)* "'"
    /// ```
    /// (`Reference` is something like `&quot;`, but we don't care about
    /// escaped characters at this level)
    ///
    /// # Parameters
    /// - `buf`: Buffer that could be filled from an input (`Self`) and
    ///   from which [events] could borrow their data
    /// - `position`: Will be increased by amount of bytes consumed
    ///
    /// [defined]: https://www.w3.org/TR/xml11/#NT-AttValue
    /// [events]: crate::events::Event
    fn read_element(&mut self, buf: B, position: &mut usize) -> Result<Option<&'r [u8]>>;

    fn skip_whitespace(&mut self, position: &mut usize) -> Result<()>;

    fn skip_one(&mut self, byte: u8, position: &mut usize) -> Result<bool>;

    fn peek_one(&mut self) -> Result<Option<u8>>;
}

/// Implementation of `XmlSource` for any `BufRead` reader using a user-given
/// `Vec<u8>` as buffer that will be borrowed by events.
impl<'b, R: BufRead> XmlSource<'b, &'b mut Vec<u8>> for R {
    #[inline]
    fn read_bytes_until(
        &mut self,
        byte: u8,
        buf: &'b mut Vec<u8>,
        position: &mut usize,
    ) -> Result<Option<&'b [u8]>> {
        let mut read = 0;
        let mut done = false;
        let start = buf.len();
        while !done {
            let used = {
                let available = match self.fill_buf() {
                    Ok(n) if n.is_empty() => break,
                    Ok(n) => n,
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        *position += read;
                        return Err(Error::Io(e));
                    }
                };

                match memchr::memchr(byte, available) {
                    Some(i) => {
                        buf.extend_from_slice(&available[..i]);
                        done = true;
                        i + 1
                    }
                    None => {
                        buf.extend_from_slice(available);
                        available.len()
                    }
                }
            };
            self.consume(used);
            read += used;
        }
        *position += read;

        if read == 0 {
            Ok(None)
        } else {
            Ok(Some(&buf[start..]))
        }
    }

    fn read_bang_element(
        &mut self,
        buf: &'b mut Vec<u8>,
        position: &mut usize,
    ) -> Result<Option<(BangType, &'b [u8])>> {
        // Peeked one bang ('!') before being called, so it's guaranteed to
        // start with it.
        let start = buf.len();
        let mut read = 1;
        buf.push(b'!');
        self.consume(1);

        let bang_type = BangType::new(self.peek_one()?)?;

        loop {
            match self.fill_buf() {
                // Note: Do not update position, so the error points to
                // somewhere sane rather than at the EOF
                Ok(n) if n.is_empty() => return Err(bang_type.to_err()),
                Ok(available) => {
                    if let Some((consumed, used)) = bang_type.parse(available, read) {
                        buf.extend_from_slice(consumed);

                        self.consume(used);
                        read += used;

                        *position += read;
                        break;
                    } else {
                        buf.extend_from_slice(available);

                        let used = available.len();
                        self.consume(used);
                        read += used;
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    *position += read;
                    return Err(Error::Io(e));
                }
            }
        }

        if read == 0 {
            Ok(None)
        } else {
            Ok(Some((bang_type, &buf[start..])))
        }
    }

    #[inline]
    fn read_element(
        &mut self,
        buf: &'b mut Vec<u8>,
        position: &mut usize,
    ) -> Result<Option<&'b [u8]>> {
        let mut state = ReadElementState::Elem;
        let mut read = 0;

        let start = buf.len();
        loop {
            match self.fill_buf() {
                Ok(n) if n.is_empty() => break,
                Ok(available) => {
                    if let Some((consumed, used)) = state.change(available) {
                        buf.extend_from_slice(consumed);

                        self.consume(used);
                        read += used;

                        *position += read;
                        break;
                    } else {
                        buf.extend_from_slice(available);

                        let used = available.len();
                        self.consume(used);
                        read += used;
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    *position += read;
                    return Err(Error::Io(e));
                }
            };
        }

        if read == 0 {
            Ok(None)
        } else {
            Ok(Some(&buf[start..]))
        }
    }

    /// Consume and discard all the whitespace until the next non-whitespace
    /// character or EOF.
    fn skip_whitespace(&mut self, position: &mut usize) -> Result<()> {
        loop {
            break match self.fill_buf() {
                Ok(n) => {
                    let count = n.iter().position(|b| !is_whitespace(*b)).unwrap_or(n.len());
                    if count > 0 {
                        self.consume(count);
                        *position += count;
                        continue;
                    } else {
                        Ok(())
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => Err(Error::Io(e)),
            };
        }
    }

    /// Consume and discard one character if it matches the given byte. Return
    /// true if it matched.
    fn skip_one(&mut self, byte: u8, position: &mut usize) -> Result<bool> {
        match self.peek_one()? {
            Some(b) if b == byte => {
                *position += 1;
                self.consume(1);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Return one character without consuming it, so that future `read_*` calls
    /// will still include it. On EOF, return None.
    fn peek_one(&mut self) -> Result<Option<u8>> {
        loop {
            break match self.fill_buf() {
                Ok(n) if n.is_empty() => Ok(None),
                Ok(n) => Ok(Some(n[0])),
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => Err(Error::Io(e)),
            };
        }
    }
}

/// Implementation of `XmlSource` for `&[u8]` reader using a `Self` as buffer
/// that will be borrowed by events. This implementation provides a zero-copy deserialization
impl<'a> XmlSource<'a, ()> for &'a [u8] {
    fn read_bytes_until(
        &mut self,
        byte: u8,
        _buf: (),
        position: &mut usize,
    ) -> Result<Option<&'a [u8]>> {
        if self.is_empty() {
            return Ok(None);
        }

        Ok(Some(if let Some(i) = memchr::memchr(byte, self) {
            *position += i + 1;
            let bytes = &self[..i];
            *self = &self[i + 1..];
            bytes
        } else {
            *position += self.len();
            let bytes = &self[..];
            *self = &[];
            bytes
        }))
    }

    fn read_bang_element(
        &mut self,
        _buf: (),
        position: &mut usize,
    ) -> Result<Option<(BangType, &'a [u8])>> {
        // Peeked one bang ('!') before being called, so it's guaranteed to
        // start with it.
        debug_assert_eq!(self[0], b'!');

        let bang_type = BangType::new(self[1..].first().copied())?;

        if let Some((bytes, i)) = bang_type.parse(self, 0) {
            *position += i;
            *self = &self[i..];
            return Ok(Some((bang_type, bytes)));
        }

        // Note: Do not update position, so the error points to
        // somewhere sane rather than at the EOF
        Err(bang_type.to_err())
    }

    fn read_element(&mut self, _buf: (), position: &mut usize) -> Result<Option<&'a [u8]>> {
        if self.is_empty() {
            return Ok(None);
        }

        let mut state = ReadElementState::Elem;

        if let Some((bytes, i)) = state.change(self) {
            *position += i;
            *self = &self[i..];
            return Ok(Some(bytes));
        }

        // Note: Do not update position, so the error points to a sane place
        // rather than at the EOF.
        Err(Error::UnexpectedEof("Element".to_string()))

        // FIXME: Figure out why the other one works without UnexpectedEof
    }

    fn skip_whitespace(&mut self, position: &mut usize) -> Result<()> {
        let whitespaces = self
            .iter()
            .position(|b| !is_whitespace(*b))
            .unwrap_or(self.len());
        *position += whitespaces;
        *self = &self[whitespaces..];
        Ok(())
    }

    fn skip_one(&mut self, byte: u8, position: &mut usize) -> Result<bool> {
        if self.first() == Some(&byte) {
            *self = &self[1..];
            *position += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn peek_one(&mut self) -> Result<Option<u8>> {
        Ok(self.first().copied())
    }
}
