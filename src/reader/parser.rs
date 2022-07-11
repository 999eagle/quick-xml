use std::str::from_utf8;

use delegate::delegate;
#[cfg(feature = "encoding")]
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

use crate::{
    events::{BytesCData, BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    name::{LocalName, NamespaceResolver, QName, ResolveResult},
    reader::{builder::InnerParserBuilder, is_whitespace, BangType, TagState},
    Error, Result,
};

#[cfg(feature = "encoding")]
use super::EncodingRef;

mod sealed {
    /// Seal trait to make sure no other parsers can be implemented.
    pub trait Sealed {}

    impl Sealed for super::DefaultParser {}
    impl Sealed for super::NamespacedParser {}
}

/// Trait defining functions for a generic XML parser.
///
/// This trait is meant for internal use. It's only `pub` to allow [`DefaultParser`] and
/// [`NamespacedParser`] to be `pub` to be able to name concrete implementations of
/// [`Reader`](super::Reader) to be named in external crates.
pub trait Parser: sealed::Sealed {
    /// Build a new parser from the given builder.
    fn from_builder(builder: InnerParserBuilder) -> Self;
    /// Get the current buffer position.
    fn buf_position(&self) -> usize;
    /// Get a mutable reference to the current buffer position.
    fn mut_buf_position(&mut self) -> &mut usize;
    /// Set the current buffer position.
    fn set_buf_position(&mut self, buf_position: usize);
    /// Get the current tag state.
    fn tag_state(&self) -> TagState;
    /// Set the current tag state.
    fn set_tag_state(&mut self, tag_state: TagState);
    /// Get whether empty elements should be expanded into a start and an end event.
    fn expand_empty_elements(&self) -> bool;
    /// Get whether whitespace in front of character events should be trimmed.
    fn trim_text_start(&self) -> bool;
    /// Get whether whitespace in the end of character events should be trimmed.
    fn trim_text_end(&self) -> bool;
    /// Get whether whitespace after markup names in end tags should be trimmed.
    fn trim_markup_names_in_closing_tags(&self) -> bool;
    /// Get whether end tag names should be checked against the corresponding start tag.
    fn check_end_names(&self) -> bool;
    /// Get whether comments should be validated.
    fn check_comments(&self) -> bool;
    /// Get a mutable reference to the buffer of opened but not closed start tags.
    fn mut_opened_buffer(&mut self) -> &mut Vec<u8>;
    /// Get a mutable reference to the buffer indexing the buffer of opened start tags.
    fn mut_opened_starts(&mut self) -> &mut Vec<usize>;
    /// Get the current encoding.
    #[cfg(feature = "encoding")]
    fn encoding(&self) -> EncodingRef;
    /// Set the current encoding.
    #[cfg(feature = "encoding")]
    fn set_encoding(&mut self, encoding: EncodingRef);

    /// reads `BytesElement` starting with a `!`,
    /// return `Comment`, `CData` or `DocType` event
    fn read_bang<'b>(&mut self, bang_type: BangType, buf: &'b [u8]) -> Result<Event<'b>> {
        let uncased_starts_with = |string: &[u8], prefix: &[u8]| {
            string.len() >= prefix.len() && string[..prefix.len()].eq_ignore_ascii_case(prefix)
        };

        let len = buf.len();
        match bang_type {
            BangType::Comment if buf.starts_with(b"!--") => {
                if self.check_comments() {
                    // search if '--' not in comments
                    if let Some(p) = memchr::memchr_iter(b'-', &buf[3..len - 2])
                        .position(|p| buf[3 + p + 1] == b'-')
                    {
                        *self.mut_buf_position() += len - p;
                        return Err(Error::UnexpectedToken("--".to_string()));
                    }
                }
                Ok(Event::Comment(BytesText::from_escaped(&buf[3..len - 2])))
            }
            BangType::CData if uncased_starts_with(buf, b"![CDATA[") => {
                Ok(Event::CData(BytesCData::new(&buf[8..])))
            }
            BangType::DocType if uncased_starts_with(buf, b"!DOCTYPE") => {
                let start = buf[8..]
                    .iter()
                    .position(|b| !is_whitespace(*b))
                    .unwrap_or_else(|| len - 8);
                debug_assert!(start < len - 8, "DocType must have a name");
                Ok(Event::DocType(BytesText::from_escaped(&buf[8 + start..])))
            }
            _ => Err(bang_type.to_err()),
        }
    }

    /// reads `BytesElement` starting with a `/`,
    /// if `self.check_end_names`, checks that element matches last opened element
    /// return `End` event
    fn read_end<'b>(&mut self, buf: &'b [u8]) -> Result<Event<'b>> {
        // XML standard permits whitespaces after the markup name in closing tags.
        // Let's strip them from the buffer before comparing tag names.
        let name = if self.trim_markup_names_in_closing_tags() {
            if let Some(pos_end_name) = buf[1..].iter().rposition(|&b| !b.is_ascii_whitespace()) {
                let (name, _) = buf[1..].split_at(pos_end_name + 1);
                name
            } else {
                &buf[1..]
            }
        } else {
            &buf[1..]
        };
        if self.check_end_names() {
            let mismatch_err = |expected: &[u8], found: &[u8], buf_position: &mut usize| {
                *buf_position -= buf.len();
                Err(Error::EndEventMismatch {
                    expected: from_utf8(expected).unwrap_or("").to_owned(),
                    found: from_utf8(found).unwrap_or("").to_owned(),
                })
            };
            match self.mut_opened_starts().pop() {
                Some(start) => {
                    let mut position = self.buf_position();
                    let expected = &self.mut_opened_buffer()[start..];
                    if name != expected {
                        let result = mismatch_err(expected, name, &mut position);
                        self.set_buf_position(position);
                        result
                    } else {
                        self.mut_opened_buffer().truncate(start);
                        Ok(Event::End(BytesEnd::borrowed(name)))
                    }
                }
                None => mismatch_err(b"", &buf[1..], self.mut_buf_position()),
            }
        } else {
            Ok(Event::End(BytesEnd::borrowed(name)))
        }
    }

    /// reads `BytesElement` starting with a `?`,
    /// return `Decl` or `PI` event
    fn read_question_mark<'b>(&mut self, buf: &'b [u8]) -> Result<Event<'b>> {
        let len = buf.len();
        if len > 2 && buf[len - 1] == b'?' {
            if len > 5 && &buf[1..4] == b"xml" && is_whitespace(buf[4]) {
                let event = BytesDecl::from_start(BytesStart::borrowed(&buf[1..len - 1], 3));

                // Try getting encoding from the declaration event
                #[cfg(feature = "encoding")]
                if self.encoding().can_be_refined() {
                    if let Some(encoding) = event.encoder() {
                        self.set_encoding(EncodingRef::XmlDetected(encoding));
                    }
                }

                Ok(Event::Decl(event))
            } else {
                Ok(Event::PI(BytesText::from_escaped(&buf[1..len - 1])))
            }
        } else {
            *self.mut_buf_position() -= len;
            Err(Error::UnexpectedEof("XmlDecl".to_string()))
        }
    }

    /// closes the current tag and returns an `End` event
    #[inline]
    fn close_expanded_empty(&mut self) -> Result<Event<'static>> {
        self.set_tag_state(TagState::Closed);
        let start = self.mut_opened_starts().pop().unwrap();
        let name = self.mut_opened_buffer().split_off(start);
        Ok(Event::End(BytesEnd::owned(name)))
    }

    /// reads `BytesElement` starting with any character except `/`, `!` or ``?`
    /// return `Start` or `Empty` event
    fn read_start<'b>(&mut self, buf: &'b [u8]) -> Result<Event<'b>> {
        // TODO: do this directly when reading bufreader ...
        let len = buf.len();
        let name_end = buf.iter().position(|&b| is_whitespace(b)).unwrap_or(len);
        if let Some(&b'/') = buf.last() {
            let end = if name_end < len { name_end } else { len - 1 };
            let buf_len = self.mut_opened_buffer().len();
            if self.expand_empty_elements() {
                self.set_tag_state(TagState::Empty);
                self.mut_opened_starts().push(buf_len);
                self.mut_opened_buffer().extend(&buf[..end]);
                Ok(Event::Start(BytesStart::borrowed(&buf[..len - 1], end)))
            } else {
                Ok(Event::Empty(BytesStart::borrowed(&buf[..len - 1], end)))
            }
        } else {
            let buf_len = self.mut_opened_buffer().len();
            if self.check_end_names() {
                self.mut_opened_starts().push(buf_len);
                self.mut_opened_buffer().extend(&buf[..name_end]);
            }
            Ok(Event::Start(BytesStart::borrowed(buf, name_end)))
        }
    }
}

/// Default parser implementing the [`Parser`] trait. Does not handle namespaced elements.
#[derive(Clone)]
pub struct DefaultParser {
    /// current buffer position, useful for debugging errors
    buf_position: usize,
    /// current state Open/Close
    tag_state: TagState,
    /// expand empty element into an opening and closing element
    expand_empty_elements: bool,
    /// trims leading whitespace in Text events, skip the element if text is empty
    trim_text_start: bool,
    /// trims trailing whitespace in Text events.
    trim_text_end: bool,
    /// trims trailing whitespaces from markup names in closing tags `</a >`
    trim_markup_names_in_closing_tags: bool,
    /// check if End nodes match last Start node
    check_end_names: bool,
    /// check if comments contains `--` (false per default)
    check_comments: bool,
    /// All currently Started elements which didn't have a matching
    /// End element yet.
    ///
    /// For an XML
    ///
    /// ```xml
    /// <root><one/><inner attr="value">|<tag></inner></root>
    /// ```
    /// when cursor at the `|` position buffer contains:
    ///
    /// ```text
    /// rootinner
    /// ^   ^
    /// ```
    ///
    /// The `^` symbols shows which positions stored in the [`Self::opened_starts`]
    /// (0 and 4 in that case).
    opened_buffer: Vec<u8>,
    /// Opened name start indexes into [`Self::opened_buffer`]. See documentation
    /// for that field for details
    opened_starts: Vec<usize>,

    #[cfg(feature = "encoding")]
    /// Reference to the encoding used to read an XML
    encoding: EncodingRef,
}

/// Namespaced parser implementing the [`Parser`] trait. Handles namespaced elements.
#[derive(Clone)]
pub struct NamespacedParser {
    /// Inner parser used by this
    pub(super) inner: DefaultParser,
    /// A buffer to manage namespaces
    pub(super) ns_resolver: NamespaceResolver,
    /// For `Empty` events keep the 'scope' of the namespace on the stack artificially. That way, the
    /// consumer has a chance to use `resolve` in the context of the empty element. We perform the
    /// pop as the first operation in the next `next()` call.
    pub(super) pending_pop: bool,
}

/// Builder
impl DefaultParser {
    /// Creates a new [`DefaultParser`] with default settings.
    ///
    /// To change these, use a [`ParserBuilder`](super::ParserBuilder) instead.
    pub fn new() -> Self {
        Self {
            opened_buffer: Vec::new(),
            opened_starts: Vec::new(),
            tag_state: TagState::Init,
            expand_empty_elements: false,
            trim_text_start: false,
            trim_text_end: false,
            trim_markup_names_in_closing_tags: true,
            check_end_names: true,
            buf_position: 0,
            check_comments: false,

            #[cfg(feature = "encoding")]
            encoding: EncodingRef::Implicit(UTF_8),
        }
    }
}

/// Builder
impl NamespacedParser {
    /// Creates a new [`NamespacedParser`] with default settings.
    ///
    /// To change these, use a [`ParserBuilder`](super::ParserBuilder) instead.
    pub fn from_parser(parser: DefaultParser) -> Self {
        Self {
            inner: parser,
            ns_resolver: NamespaceResolver::default(),
            pending_pop: false,
        }
    }
}

impl Parser for DefaultParser {
    fn from_builder(builder: InnerParserBuilder) -> Self {
        Self {
            opened_buffer: Vec::new(),
            opened_starts: Vec::new(),
            tag_state: TagState::Init,
            buf_position: 0,

            expand_empty_elements: builder.expand_empty_elements,
            trim_text_start: builder.trim_text_start,
            trim_text_end: builder.trim_text_end,
            trim_markup_names_in_closing_tags: builder.trim_markup_names_in_closing_tags,
            check_end_names: builder.check_end_names,
            check_comments: builder.check_comments,

            #[cfg(feature = "encoding")]
            encoding: EncodingRef::Implicit(UTF_8),
        }
    }

    #[inline]
    fn buf_position(&self) -> usize {
        self.buf_position
    }

    #[inline]
    fn mut_buf_position(&mut self) -> &mut usize {
        &mut self.buf_position
    }

    #[inline]
    fn set_buf_position(&mut self, buf_position: usize) {
        self.buf_position = buf_position
    }

    #[inline]
    fn tag_state(&self) -> TagState {
        self.tag_state
    }

    #[inline]
    fn set_tag_state(&mut self, tag_state: TagState) {
        self.tag_state = tag_state
    }

    #[inline]
    fn expand_empty_elements(&self) -> bool {
        self.expand_empty_elements
    }

    #[inline]
    fn trim_text_start(&self) -> bool {
        self.trim_text_start
    }

    #[inline]
    fn trim_text_end(&self) -> bool {
        self.trim_text_end
    }

    #[inline]
    fn trim_markup_names_in_closing_tags(&self) -> bool {
        self.trim_markup_names_in_closing_tags
    }

    #[inline]
    fn check_end_names(&self) -> bool {
        self.check_end_names
    }

    #[inline]
    fn check_comments(&self) -> bool {
        self.check_comments
    }

    #[inline]
    fn mut_opened_buffer(&mut self) -> &mut Vec<u8> {
        &mut self.opened_buffer
    }

    #[inline]
    fn mut_opened_starts(&mut self) -> &mut Vec<usize> {
        &mut self.opened_starts
    }

    #[cfg(feature = "encoding")]
    #[inline]
    fn encoding(&self) -> EncodingRef {
        self.encoding
    }

    #[cfg(feature = "encoding")]
    #[inline]
    fn set_encoding(&mut self, encoding: EncodingRef) {
        self.encoding = encoding
    }
}

impl Parser for NamespacedParser {
    delegate! {
        to self.inner {
            fn buf_position(&self) -> usize;
            fn mut_buf_position(&mut self) -> &mut usize;
            fn set_buf_position(&mut self, buf_position: usize);
            fn tag_state(&self) -> TagState;
            fn set_tag_state(&mut self, tag_state: TagState);
            fn expand_empty_elements(&self) -> bool;
            fn trim_text_start(&self) -> bool;
            fn trim_text_end(&self) -> bool;
            fn trim_markup_names_in_closing_tags(&self) -> bool;
            fn check_end_names(&self) -> bool;
            fn check_comments(&self) -> bool;
            fn mut_opened_buffer(&mut self) -> &mut Vec<u8>;
            fn mut_opened_starts(&mut self) -> &mut Vec<usize>;
            #[cfg(feature = "encoding")]
            fn encoding(&self) -> EncodingRef;
            #[cfg(feature = "encoding")]
            fn set_encoding(&mut self, encoding: EncodingRef);
        }
    }

    fn from_builder(builder: InnerParserBuilder) -> Self {
        let inner = DefaultParser::from_builder(builder);
        Self {
            inner,
            ns_resolver: NamespaceResolver::default(),
            pending_pop: false,
        }
    }
}

/// Getters
impl NamespacedParser {
    /// Resolves a potentially qualified **event name** into (namespace name, local name).
    ///
    /// *Qualified* attribute names have the form `prefix:local-name` where the`prefix` is defined
    /// on any containing XML element via `xmlns:prefix="the:namespace:uri"`. The namespace prefix
    /// can be defined on the same element as the attribute in question.
    ///
    /// *Unqualified* event inherits the current *default namespace*.
    ///
    /// # Lifetimes
    ///
    /// - `'n`: lifetime of an element name
    /// - `'ns`: lifetime of a namespaces buffer, where all found namespaces are stored
    #[inline]
    pub fn event_namespace<'n, 'ns>(
        &self,
        name: QName<'n>,
        namespace_buffer: &'ns [u8],
    ) -> (ResolveResult<'ns>, LocalName<'n>) {
        self.ns_resolver.resolve(name, namespace_buffer, true)
    }

    /// Resolves a potentially qualified **attribute name** into (namespace name, local name).
    ///
    /// *Qualified* attribute names have the form `prefix:local-name` where the`prefix` is defined
    /// on any containing XML element via `xmlns:prefix="the:namespace:uri"`. The namespace prefix
    /// can be defined on the same element as the attribute in question.
    ///
    /// *Unqualified* attribute names do *not* inherit the current *default namespace*.
    ///
    /// # Lifetimes
    ///
    /// - `'n`: lifetime of an attribute
    /// - `'ns`: lifetime of a namespaces buffer, where all found namespaces are stored
    #[inline]
    pub fn attribute_namespace<'n, 'ns>(
        &self,
        name: QName<'n>,
        namespace_buffer: &'ns [u8],
    ) -> (ResolveResult<'ns>, LocalName<'n>) {
        self.ns_resolver.resolve(name, namespace_buffer, false)
    }
}

/// Private methods
impl DefaultParser {}
