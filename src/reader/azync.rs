//! Module for async-specific reader code.

use std::{future::Future, io, pin::Pin};

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::{Error, Result};

use super::{is_whitespace, BangType, ReadElementState};

/// Represents an async input for a reader that can return borrowed data.
///
/// Async equivalent of [`XmlSource`](super::XmlSource)
pub(super) trait AsyncXmlSource<'buf, B> {
    /// Read input until `byte` is found or end of input is reached.
    ///
    /// Equivalent to:
    /// ```ignore
    /// async fn read_bytes_until(&mut self, byte: u8, buf: B, position: &mut usize) -> Result<Option<&[u8]>>;
    /// ```
    ///
    /// See also [`XmlSource::read_bytes_until`](super::XmlSource::read_bytes_until).
    fn read_bytes_until<'_self, 'pos, 'func>(
        &'_self mut self,
        byte: u8,
        buf: B,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<&'buf [u8]>>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func;

    /// Equivalent to:
    /// ```ignore
    /// async fn read_bang_element(&mut self, buf: B, position: &mut usize) -> Result<Option<(BangType, &[u8])>>;
    /// ```
    fn read_bang_element<'_self, 'pos, 'func>(
        &'_self mut self,
        buf: B,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(BangType, &'buf [u8])>>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func;

    /// Equivalent to:
    /// ```ignore
    /// async fn read_element(&mut self, buf: B, position: &mut usize) -> Result<Option<&[u8]>>;
    /// ```
    fn read_element<'_self, 'pos, 'func>(
        &'_self mut self,
        buf: B,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<&'buf [u8]>>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func;

    /// Equivalent to:
    /// ```ignore
    /// async fn skip_whitespace(&mut self, position: &mut usize) -> Result<()>;
    /// ```
    fn skip_whitespace<'_self, 'pos, 'func>(
        &'_self mut self,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func;

    /// Equivalent to:
    /// ```ignore
    /// async fn skip_one(&mut self, byte: u8, position: &mut usize) -> Result<bool>;
    /// ```
    fn skip_one<'_self, 'pos, 'func>(
        &'_self mut self,
        byte: u8,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func;

    /// Equivalent to:
    /// ```ignore
    /// async fn peek_one(&mut self) -> Result<Option<u8>>;
    /// ```
    fn peek_one<'_self, 'func>(
        &'_self mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<u8>>> + Send + 'func>>
    where
        '_self: 'func,
        'buf: 'func;
}

impl<'buf, R: AsyncBufRead + Unpin + Send + 'buf> AsyncXmlSource<'buf, &'buf mut Vec<u8>> for R {
    fn read_bytes_until<'a, 'b, 'func>(
        &'a mut self,
        byte: u8,
        buf: &'buf mut Vec<u8>,
        position: &'b mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<&'buf [u8]>>> + Send + 'func>>
    where
        'a: 'func,
        'b: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            let mut read = 0;
            let mut done = false;
            let start = buf.len();
            while !done {
                let used = {
                    let available = match self.fill_buf().await {
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
        })
    }

    fn read_bang_element<'_self, 'pos, 'func>(
        &'_self mut self,
        buf: &'buf mut Vec<u8>,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(BangType, &'buf [u8])>>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            // Peeked one bang ('!') before being called, so it's guaranteed to
            // start with it.
            let start = buf.len();
            let mut read = 1;
            buf.push(b'!');
            self.consume(1);

            let bang_type = BangType::new(self.peek_one().await?)?;

            loop {
                match self.fill_buf().await {
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
        })
    }

    fn read_element<'_self, 'pos, 'func>(
        &'_self mut self,
        buf: &'buf mut Vec<u8>,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<Option<&'buf [u8]>>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            let mut state = ReadElementState::Elem;
            let mut read = 0;

            let start = buf.len();
            loop {
                match self.fill_buf().await {
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
        })
    }

    fn skip_whitespace<'_self, 'pos, 'func>(
        &'_self mut self,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            loop {
                break match self.fill_buf().await {
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
        })
    }

    fn skip_one<'_self, 'pos, 'func>(
        &'_self mut self,
        byte: u8,
        position: &'pos mut usize,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'func>>
    where
        '_self: 'func,
        'pos: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            match self.peek_one().await? {
                Some(b) if b == byte => {
                    *position += 1;
                    self.consume(1);
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    }

    fn peek_one<'_self, 'func>(
        &'_self mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<u8>>> + Send + 'func>>
    where
        '_self: 'func,
        'buf: 'func,
    {
        Box::pin(async move {
            loop {
                break match self.fill_buf().await {
                    Ok(n) if n.is_empty() => Ok(None),
                    Ok(n) => Ok(Some(n[0])),
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => Err(Error::Io(e)),
                };
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_xml_read_bytes_until() {
        let mut position = 0;
        let mut data = b"abc*123".as_ref();
        let mut buf = Vec::new();

        let result = data
            .read_bytes_until(b'*', &mut buf, &mut position)
            .await
            .unwrap();
        assert_eq!(result, Some(b"abc".as_ref()));
        assert_eq!(buf, b"abc");
        assert_eq!(position, 4);
    }

    #[tokio::test]
    async fn test_xml_peek_one() {
        let mut data = b"abc*123".as_ref();

        let result = data.peek_one().await.unwrap();
        assert_eq!(result, Some(b'a'));
    }

    #[tokio::test]
    async fn test_xml_read_bang() {
        let mut position = 1;
        let source = b"<!DOCTYPE test>";
        let mut data = source.as_ref();
        let mut buf = Vec::new();

        data.fill_buf().await.unwrap();
        data.consume(1);

        let result = data.peek_one().await.unwrap();
        assert_eq!(result, Some(b'!'));

        let result = data
            .read_bang_element(&mut buf, &mut position)
            .await
            .unwrap();
        assert_eq!(result, Some((BangType::DocType, b"!DOCTYPE test".as_ref())));
        assert_eq!(buf, b"!DOCTYPE test".as_ref());
        assert_eq!(position, source.len());
    }

    #[tokio::test]
    async fn test_xml_read_elem() {
        let mut position = 1;
        let source = b"<element attribute=\"something\">";
        let mut data = source.as_ref();
        let mut buf = Vec::new();

        data.fill_buf().await.unwrap();
        data.consume(1);

        let result = data.read_element(&mut buf, &mut position).await.unwrap();
        assert_eq!(result, Some(b"element attribute=\"something\"".as_ref()));
        assert_eq!(buf, b"element attribute=\"something\"".as_ref());
        assert_eq!(position, source.len());
    }
}
