use std::marker::PhantomData;
use std::{fs::File, io::BufReader, path::Path};

#[cfg(feature = "encoding")]
use encoding_rs::UTF_8;

use crate::{Error, Parser, Reader, Result};

use super::parser::{DefaultParser, NamespacedParser};
#[cfg(feature = "encoding")]
use super::EncodingRef;

pub struct InnerParserBuilder {
    pub(super) expand_empty_elements: bool,
    pub(super) trim_text_start: bool,
    pub(super) trim_text_end: bool,
    pub(super) trim_markup_names_in_closing_tags: bool,
    pub(super) check_end_names: bool,
    pub(super) check_comments: bool,
}

impl Default for InnerParserBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl InnerParserBuilder {
    fn new() -> Self {
        Self {
            expand_empty_elements: false,
            trim_text_start: false,
            trim_text_end: false,
            trim_markup_names_in_closing_tags: true,
            check_end_names: true,
            check_comments: false,
        }
    }
}

/// Builder for configuring a new parser.
pub struct ParserBuilder<P> {
    inner: InnerParserBuilder,
    _phantom: PhantomData<P>,
}

impl ParserBuilder<DefaultParser> {
    /// Create a new default [`ParserBuilder`].
    pub fn new() -> Self {
        Self {
            inner: InnerParserBuilder::new(),
            _phantom: PhantomData,
        }
    }

    /// Changes this builder to return a parser that does handle namespaces.
    pub fn with_namespace(self) -> ParserBuilder<NamespacedParser> {
        ParserBuilder {
            inner: self.inner,
            _phantom: PhantomData,
        }
    }

    /// Changes this builder to return a parser that does *not* handle namespaces.
    pub fn without_namespace(self) -> Self {
        self
    }
}

impl ParserBuilder<NamespacedParser> {
    /// Create a new default [`ParserBuilder`].
    pub fn new() -> Self {
        Self {
            inner: InnerParserBuilder::new(),
            _phantom: PhantomData,
        }
    }

    /// Changes this builder to return a parser that does handle namespaces.
    pub fn with_namespace(self) -> Self {
        self
    }

    /// Changes this builder to return a parser that does *not* handle namespaces.
    pub fn without_namespace(self) -> ParserBuilder<DefaultParser> {
        ParserBuilder {
            inner: self.inner,
            _phantom: PhantomData,
        }
    }
}

impl<P: Parser> ParserBuilder<P> {
    /// Builds a new [`Parser`] from this configuration.
    pub fn build(self) -> P {
        P::from_builder(self.inner)
    }
}

/// Builder for configuring a new reader based on the underlying parser.
pub struct ReaderBuilder<P> {
    parser: ParserBuilder<P>,
}

impl ReaderBuilder<DefaultParser> {
    /// Create a new default [`ReaderBuilder`].
    pub fn new() -> Self {
        Self {
            parser: ParserBuilder::<DefaultParser>::new(),
        }
    }

    /// Changes this builder to return a reader that does handle namespaces.
    pub fn with_namespace(self) -> ReaderBuilder<NamespacedParser> {
        ReaderBuilder {
            parser: self.parser.with_namespace(),
        }
    }

    /// Changes this builder to return a reader that does *not* handle namespaces.
    pub fn without_namespace(self) -> Self {
        self
    }
}

impl ReaderBuilder<NamespacedParser> {
    /// Create a new default [`ReaderBuilder`].
    pub fn new() -> Self {
        Self {
            parser: ParserBuilder::<NamespacedParser>::new(),
        }
    }

    /// Changes this builder to return a reader that does handle namespaces.
    pub fn with_namespace(self) -> Self {
        self
    }

    /// Changes this builder to return a parser that does *not* handle namespaces.
    pub fn without_namespace(self) -> ReaderBuilder<DefaultParser> {
        ReaderBuilder {
            parser: self.parser.without_namespace(),
        }
    }
}

impl<P: Parser> ReaderBuilder<P> {
    /// Create a [`ReaderBuilder`] from the given [`ParserBuilder`].
    pub fn from_parser(parser: ParserBuilder<P>) -> Self {
        Self { parser }
    }

    /// Builds a new [`Reader`] from this configuration and the given reader.
    pub fn into_reader<R>(self, reader: R) -> Reader<R, P> {
        Reader {
            reader,
            parser: self.parser.build(),
        }
    }

    /// Builds a new [`Reader`] from this configuration reading from the given string slice.
    pub fn into_str_reader<'b>(self, str: &'b str) -> Reader<&'b [u8], P> {
        #[cfg_attr(not(feature = "encoding"), allow(unused_mut))]
        let mut reader = self.into_reader(str.as_bytes());
        // Rust strings are guaranteed to be UTF-8, so lock the encoding
        #[cfg(feature = "encoding")]
        {
            reader.parser.set_encoding(EncodingRef::Explicit(UTF_8));
        }
        reader
    }

    /// Creates an XML reader from a file path.
    pub fn into_file_reader<R: AsRef<Path>>(self, path: R) -> Result<Reader<BufReader<File>, P>> {
        let file = File::open(path).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        Ok(self.into_reader(reader))
    }
}

macro_rules! impl_build_methods {
    ($builder:ident < $p:ident >, $($var:ident).+) => {
        impl<$p> $builder<$p> {
            /// Changes whether empty elements should be split into an `Open` and a `Close` event.
            ///
            /// When set to `true`, all [`Empty`] events produced by a self-closing tag like `<tag/>` are
            /// expanded into a [`Start`] event followed by an [`End`] event. When set to `false` (the
            /// default), those tags are represented by an [`Empty`] event instead.
            ///
            /// Note, that setting this to `true` will lead to additional allocates that
            /// needed to store tag name for an [`End`] event. There is no additional
            /// allocation, however, if [`Self::check_end_names()`] is also set.
            ///
            /// (`false` by default)
            ///
            /// [`Empty`]: events/enum.Event.html#variant.Empty
            /// [`Start`]: events/enum.Event.html#variant.Start
            /// [`End`]: events/enum.Event.html#variant.End
            pub fn expand_empty_elements(mut self, val: bool) -> Self {
                self.$($var).+.expand_empty_elements = val;
                self
            }

            /// Changes whether whitespace before and after character data should be removed.
            ///
            /// When set to `true`, all [`Text`] events are trimmed. If they are empty, no event will be
            /// pushed.
            ///
            /// (`false` by default)
            ///
            /// [`Text`]: events/enum.Event.html#variant.Text
            pub fn trim_text(mut self, val: bool) -> Self {
                self.$($var).+.trim_text_start = val;
                self.$($var).+.trim_text_end = val;
                self
            }

            /// Changes whether whitespace after character data should be removed.
            ///
            /// When set to `true`, trailing whitespace is trimmed in [`Text`] events.
            ///
            /// (`false` by default)
            ///
            /// [`Text`]: events/enum.Event.html#variant.Text
            pub fn trim_text_end(mut self, val: bool) -> Self {
                self.$($var).+.trim_text_end = val;
                self
            }

            /// Changes whether trailing whitespaces after the markup name are trimmed in closing tags
            /// `</a >`.
            ///
            /// If true the emitted [`End`] event is stripped of trailing whitespace after the markup name.
            ///
            /// Note that if set to `false` and `check_end_names` is true the comparison of markup names is
            /// going to fail erronously if a closing tag contains trailing whitespaces.
            ///
            /// (`true` by default)
            ///
            /// [`End`]: events/enum.Event.html#variant.End
            pub fn trim_markup_names_in_closing_tags(mut self, val: bool) -> Self {
                self.$($var).+.trim_markup_names_in_closing_tags = val;
                self
            }

            /// Changes whether mismatched closing tag names should be detected.
            ///
            /// When set to `false`, it won't check if a closing tag matches the corresponding opening tag.
            /// For example, `<mytag></different_tag>` will be permitted.
            ///
            /// If the XML is known to be sane (already processed, etc.) this saves extra time.
            ///
            /// Note that the emitted [`End`] event will not be modified if this is disabled, ie. it will
            /// contain the data of the mismatched end tag.
            ///
            /// Note, that setting this to `true` will lead to additional allocates that
            /// needed to store tag name for an [`End`] event. There is no additional
            /// allocation, however, if [`Self::expand_empty_elements()`] is also set.
            ///
            /// (`true` by default)
            ///
            /// [`End`]: events/enum.Event.html#variant.End
            pub fn check_end_names(mut self, val: bool) -> Self {
                self.$($var).+.check_end_names = val;
                self
            }

            /// Changes whether comments should be validated.
            ///
            /// When set to `true`, every [`Comment`] event will be checked for not containing `--`, which
            /// is not allowed in XML comments. Most of the time we don't want comments at all so we don't
            /// really care about comment correctness, thus the default value is `false` to improve
            /// performance.
            ///
            /// (`false` by default)
            ///
            /// [`Comment`]: events/enum.Event.html#variant.Comment
            pub fn check_comments(mut self, val: bool) -> Self {
                self.$($var).+.check_comments = val;
                self
            }
        }
    };
}

impl_build_methods!(ParserBuilder<P>, inner);
impl_build_methods!(ReaderBuilder<P>, parser.inner);
