use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

#[cfg(feature = "encoding")]
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
#[cfg(feature = "async")]
use tokio::io::AsyncBufRead;

use crate::{name::NamespaceResolver, Error, Reader, Result};

#[cfg(feature = "encoding")]
use super::EncodingRef;
use super::{
    parser::{DefaultParser, NamespacedParser},
    TagState,
};

/// Builder for configuring a new parser.
pub struct ParserBuilder {
    expand_empty_elements: bool,
    trim_text_start: bool,
    trim_text_end: bool,
    trim_markup_names_in_closing_tags: bool,
    check_end_names: bool,
    check_comments: bool,
}

impl Default for ParserBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for configuring a new reader based on the underlying parser.
pub struct ReaderBuilder {
    parser: ParserBuilder,
}

impl Default for ReaderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserBuilder {
    /// Create a new default [`ParserBuilder`].
    pub fn new() -> Self {
        Self {
            expand_empty_elements: false,
            trim_text_start: false,
            trim_text_end: false,
            trim_markup_names_in_closing_tags: true,
            check_end_names: true,
            check_comments: false,
        }
    }

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
        self.expand_empty_elements = val;
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
        self.trim_text_start = val;
        self.trim_text_end = val;
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
        self.trim_text_end = val;
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
        self.trim_markup_names_in_closing_tags = val;
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
        self.check_end_names = val;
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
        self.check_comments = val;
        self
    }

    /// Builds a new [`DefaultParser`] from this configuration which doesn't handle namespaces.
    pub fn into_parser(self) -> DefaultParser {
        DefaultParser {
            opened_buffer: Vec::new(),
            opened_starts: Vec::new(),
            tag_state: TagState::Init,
            buf_position: 0,

            expand_empty_elements: self.expand_empty_elements,
            trim_text_start: self.trim_text_start,
            trim_text_end: self.trim_text_end,
            trim_markup_names_in_closing_tags: self.trim_markup_names_in_closing_tags,
            check_end_names: self.check_end_names,
            check_comments: self.check_comments,

            #[cfg(feature = "encoding")]
            encoding: EncodingRef::Implicit(UTF_8),
        }
    }

    /// Builds a new [`NamespacedParser`] from this configuration which does handle namespaces.
    pub fn into_namespaced_parser(self) -> NamespacedParser {
        let parser = self.into_parser();
        NamespacedParser {
            inner: parser,
            ns_resolver: NamespaceResolver::default(),
            pending_pop: false,
        }
    }
}

impl ReaderBuilder {
    /// Create a new default [`ReaderBuilder`].
    pub fn new() -> Self {
        Self {
            parser: ParserBuilder::new(),
        }
    }

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
        self.parser.expand_empty_elements = val;
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
        self.parser.trim_text_start = val;
        self.parser.trim_text_end = val;
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
        self.parser.trim_text_end = val;
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
        self.parser.trim_markup_names_in_closing_tags = val;
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
        self.parser.check_end_names = val;
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
        self.parser.check_comments = val;
        self
    }

    /// Builds a new [`Reader`] from this configuration using a non-namespaced Parser with the given inner reader.
    pub fn into_reader<R: BufRead>(self, reader: R) -> Reader<R, DefaultParser> {
        Reader {
            reader,
            parser: self.parser.into_parser(),
        }
    }

    /// Builds a new [`Reader`] from this configuration using a non-namespaced Parser reading from the given string slice.
    pub fn into_str_reader<'b>(self, str: &'b str) -> Reader<&'b [u8], DefaultParser> {
        Reader {
            reader: str.as_bytes(),
            parser: self.parser.into_parser(),
        }
    }

    /// Creates an XML reader from a file path.
    pub fn into_file_reader<P: AsRef<Path>>(
        self,
        path: P,
    ) -> Result<Reader<BufReader<File>, DefaultParser>> {
        let file = File::open(path).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        let parser = self.parser.into_parser();
        Ok(Reader::from_reader_and_parser(reader, parser))
    }

    /// Builds a new [`Reader`] from this configuration using a namespaced Parser with the given inner reader.
    pub fn into_reader_namespaced<R: BufRead>(self, reader: R) -> Reader<R, NamespacedParser> {
        Reader {
            reader,
            parser: self.parser.into_namespaced_parser(),
        }
    }

    /// Builds a new [`Reader`] from this configuration using a non-namespaced Parser reading from the given string slice.
    pub fn into_str_reader_namespaced<'b>(
        self,
        str: &'b str,
    ) -> Reader<&'b [u8], NamespacedParser> {
        Reader {
            reader: str.as_bytes(),
            parser: self.parser.into_namespaced_parser(),
        }
    }

    // #[cfg(feature = "async")]
    // pub fn into_async_reader<R: AsyncBufRead>(self, reader: R) -> Reader<R, DefaultParser> {
    //     Reader {
    //         reader,
    //         parser: self.parser.into_parser(),
    //     }
    // }

    // #[cfg(feature = "async")]
    // pub fn into_async_reader_namespaced<R: AsyncBufRead>(
    //     self,
    //     reader: R,
    // ) -> Reader<R, NamespacedParser> {
    //     Reader {
    //         reader,
    //         parser: self.parser.into_namespaced_parser(),
    //     }
    // }
}
