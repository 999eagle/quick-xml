//! A module to handle `Reader`

#[cfg(feature = "async")]
mod azync;
mod builder;
pub(crate) mod parser;
mod xml_source;

use std::borrow::Cow;
use std::io::{BufRead, BufReader};
use std::{fs::File, path::Path, str::from_utf8};

#[cfg(feature = "encoding")]
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
#[cfg(feature = "async")]
use tokio::io::AsyncBufRead;

use crate::errors::{Error, Result};
use crate::events::{BytesText, Event};
use crate::name::{LocalName, QName, ResolveResult};

#[cfg(feature = "async")]
use self::azync::AsyncXmlSource;
use self::xml_source::XmlSource;

pub use self::builder::{ParserBuilder, ReaderBuilder};
pub use self::parser::{DefaultParser, NamespacedParser, Parser};

use memchr;

/// Possible reader states. The state transition diagram (`true` and `false` shows
/// value of [`Reader::expand_empty_elements()`] option):
///
/// ```mermaid
/// flowchart LR
///   subgraph _
///     direction LR
///
///     Init   -- "(no event)"\nStartText                              --> Opened
///     Opened -- Decl, DocType, PI\nComment, CData\nStart, Empty, End --> Closed
///     Closed -- "#lt;false#gt;\n(no event)"\nText                    --> Opened
///   end
///   Closed -- "#lt;true#gt;"\nStart --> Empty
///   Empty  -- End                   --> Closed
///   _ -. Eof .-> Exit
/// ```
#[derive(Copy, Clone)]
pub enum TagState {
    /// Initial state in which reader stay after creation. Transition from that
    /// state could produce a `StartText`, `Decl`, `Comment` or `Start` event.
    /// The next state is always `Opened`. The reader will never return to this
    /// state. The event emitted during transition to `Opened` is a `StartEvent`
    /// if the first symbol not `<`, otherwise no event are emitted.
    Init,
    /// State after seeing the `<` symbol. Depending on the next symbol all other
    /// events (except `StartText`) could be generated.
    ///
    /// After generating ane event the reader moves to the `Closed` state.
    Opened,
    /// State in which reader searches the `<` symbol of a markup. All bytes before
    /// that symbol will be returned in the [`Event::Text`] event. After that
    /// the reader moves to the `Opened` state.
    Closed,
    /// This state is used only if option `expand_empty_elements` is set to `true`.
    /// Reader enters to this state when it is in a `Closed` state and emits an
    /// [`Event::Start`] event. The next event emitted will be an [`Event::End`],
    /// after which reader returned to the `Closed` state.
    Empty,
    /// Reader enters this state when `Eof` event generated or an error occurred.
    /// This is the last state, the reader stay in it forever.
    Exit,
}

/// A reference to an encoding together with information about how it was retrieved.
///
/// The state transition diagram:
///
/// ```mermaid
/// flowchart LR
///   Implicit    -- from_str       --> Explicit
///   Implicit    -- BOM            --> BomDetected
///   Implicit    -- "encoding=..." --> XmlDetected
///   BomDetected -- "encoding=..." --> XmlDetected
/// ```
#[cfg(feature = "encoding")]
#[derive(Clone, Copy)]
pub enum EncodingRef {
    /// Encoding was implicitly assumed to have a specified value. It can be refined
    /// using BOM or by the XML declaration event (`<?xml encoding=... ?>`)
    Implicit(&'static Encoding),
    /// Encoding was explicitly set to the desired value. It cannot be changed
    /// nor by BOM, nor by parsing XML declaration (`<?xml encoding=... ?>`)
    Explicit(&'static Encoding),
    /// Encoding was detected from a byte order mark (BOM) or by the first bytes
    /// of the content. It can be refined by the XML declaration event (`<?xml encoding=... ?>`)
    BomDetected(&'static Encoding),
    /// Encoding was detected using XML declaration event (`<?xml encoding=... ?>`).
    /// It can no longer change
    XmlDetected(&'static Encoding),
}
#[cfg(feature = "encoding")]
impl EncodingRef {
    #[inline]
    fn encoding(&self) -> &'static Encoding {
        match self {
            Self::Implicit(e) => e,
            Self::Explicit(e) => e,
            Self::BomDetected(e) => e,
            Self::XmlDetected(e) => e,
        }
    }
    #[inline]
    fn can_be_refined(&self) -> bool {
        match self {
            Self::Implicit(_) | Self::BomDetected(_) => true,
            Self::Explicit(_) | Self::XmlDetected(_) => false,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////

/// A low level encoding-agnostic XML event reader.
///
/// Consumes bytes and streams XML [`Event`]s.
///
/// # Examples
///
/// ```
/// use quick_xml::Reader;
/// use quick_xml::events::Event;
///
/// let xml = r#"<tag1 att1 = "test">
///                 <tag2><!--Test comment-->Test</tag2>
///                 <tag2>Test 2</tag2>
///             </tag1>"#;
/// let mut reader = Reader::builder().trim_text(true).into_str_reader(xml);
/// let mut count = 0;
/// let mut txt = Vec::new();
/// let mut buf = Vec::new();
/// loop {
///     match reader.read_event_into(&mut buf) {
///         Ok(Event::Start(ref e)) => {
///             match e.name().as_ref() {
///                 b"tag1" => println!("attributes values: {:?}",
///                                     e.attributes().map(|a| a.unwrap().value)
///                                     .collect::<Vec<_>>()),
///                 b"tag2" => count += 1,
///                 _ => (),
///             }
///         },
///         Ok(Event::Text(e)) => txt.push(e.unescape_and_decode(&reader).unwrap()),
///         Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
///         Ok(Event::Eof) => break,
///         _ => (),
///     }
///     buf.clear();
/// }
/// ```
#[derive(Clone)]
pub struct Reader<R, P> {
    pub(crate) reader: R,
    pub(crate) parser: P,
}

impl Reader<(), ()> {
    /// Create a new builder for configuring this reader.
    pub fn builder() -> ReaderBuilder<DefaultParser> {
        ReaderBuilder::<DefaultParser>::new()
    }
}

/// Builder methods
impl<R> Reader<R, DefaultParser> {
    /// Creates a `Reader` that reads from a given reader.
    pub fn from_reader(reader: R) -> Self {
        Self {
            reader,
            parser: DefaultParser::new(),
        }
    }
}

/// Builder methods
impl<R> Reader<R, NamespacedParser> {
    /// Creates a `Reader` that reads from a given reader.
    pub fn from_reader_namespaced(reader: R) -> Self {
        Self {
            reader,
            parser: NamespacedParser::from_parser(DefaultParser::new()),
        }
    }
}

/// Builder methods
impl<R, P: Parser> Reader<R, P> {
    /// Creates a `Reader` that reads from a given reader.
    pub fn from_reader_and_parser(reader: R, parser: P) -> Self {
        Self { reader, parser }
    }

    /// Creates a `Reader` that reads from a given reader.
    pub fn from_reader_and_builder(reader: R, parser: ParserBuilder<P>) -> Self {
        Self {
            reader,
            parser: parser.build(),
        }
    }
}

/// Getters
impl<R, P: Parser> Reader<R, P> {
    /// Get the encoding used to decode XML.
    #[cfg(feature = "encoding")]
    pub fn encoding(&self) -> EncodingRef {
        self.parser.encoding()
    }

    /// Get the decoder, used to decode bytes, read by this reader, to the strings.
    ///
    /// If `encoding` feature is enabled, the used encoding may change after
    /// parsing the XML declaration, otherwise encoding is fixed to UTF-8.
    ///
    /// If `encoding` feature is enabled and no encoding is specified in declaration,
    /// defaults to UTF-8.
    #[inline]
    pub fn decoder(&self) -> Decoder {
        Decoder {
            #[cfg(feature = "encoding")]
            encoding: self.parser.encoding().encoding(),
        }
    }
}

/// Getters
impl<R, P: Parser> Reader<R, P> {
    /// Consumes `Reader` returning the underlying reader
    ///
    /// Can be used to compute line and column of a parsing error position
    ///
    /// # Examples
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use std::{str, io::Cursor};
    /// use quick_xml::{Reader, DefaultParser};
    /// use quick_xml::events::Event;
    ///
    /// let xml = r#"<tag1 att1 = "test">
    ///                 <tag2><!--Test comment-->Test</tag2>
    ///                 <tag3>Test 2</tag3>
    ///             </tag1>"#;
    /// let mut reader = Reader::from_reader(Cursor::new(xml.as_bytes()));
    /// let mut buf = Vec::new();
    ///
    /// fn into_line_and_column(reader: Reader<Cursor<&[u8]>, DefaultParser>) -> (usize, usize) {
    ///     let end_pos = reader.buffer_position();
    ///     let mut cursor = reader.into_inner();
    ///     let s = String::from_utf8(cursor.into_inner()[0..end_pos].to_owned())
    ///         .expect("can't make a string");
    ///     let mut line = 1;
    ///     let mut column = 0;
    ///     for c in s.chars() {
    ///         if c == '\n' {
    ///             line += 1;
    ///             column = 0;
    ///         } else {
    ///             column += 1;
    ///         }
    ///     }
    ///     (line, column)
    /// }
    ///
    /// loop {
    ///     match reader.read_event_into(&mut buf) {
    ///         Ok(Event::Start(ref e)) => match e.name().as_ref() {
    ///             b"tag1" | b"tag2" => (),
    ///             tag => {
    ///                 assert_eq!(b"tag3", tag);
    ///                 assert_eq!((3, 22), into_line_and_column(reader));
    ///                 break;
    ///             }
    ///         },
    ///         Ok(Event::Eof) => unreachable!(),
    ///         _ => (),
    ///     }
    ///     buf.clear();
    /// }
    /// ```
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Gets a reference to the underlying reader.
    pub fn get_ref(&self) -> &R {
        &self.reader
    }

    /// Gets a mutable reference to the underlying reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    /// Gets the current byte position in the input data.
    ///
    /// Useful when debugging errors.
    pub fn buffer_position(&self) -> usize {
        // when internal state is Opened, we have actually read until '<',
        // which we don't want to show
        if let TagState::Opened = self.parser.tag_state() {
            self.parser.buf_position() - 1
        } else {
            self.parser.buf_position()
        }
    }
}

/// Read methods
impl<R: BufRead, P: Parser> Reader<R, P> {
    /// Reads the next `Event`.
    ///
    /// This is the main entry point for reading XML `Event`s.
    ///
    /// `Event`s borrow `buf` and can be converted to own their data if needed (uses `Cow`
    /// internally).
    ///
    /// Having the possibility to control the internal buffers gives you some additional benefits
    /// such as:
    ///
    /// - Reduce the number of allocations by reusing the same buffer. For constrained systems,
    ///   you can call `buf.clear()` once you are done with processing the event (typically at the
    ///   end of your loop).
    /// - Reserve the buffer length if you know the file size (using `Vec::with_capacity`).
    ///
    /// # Examples
    ///
    /// ```
    /// use quick_xml::Reader;
    /// use quick_xml::events::Event;
    ///
    /// let xml = r#"<tag1 att1 = "test">
    ///                 <tag2><!--Test comment-->Test</tag2>
    ///                 <tag2>Test 2</tag2>
    ///             </tag1>"#;
    /// let mut reader =  Reader::builder().trim_text(true).into_str_reader(xml);
    /// let mut count = 0;
    /// let mut buf = Vec::new();
    /// let mut txt = Vec::new();
    /// loop {
    ///     match reader.read_event_into(&mut buf) {
    ///         Ok(Event::Start(ref e)) => count += 1,
    ///         Ok(Event::Text(e)) => txt.push(e.unescape_and_decode(&reader).expect("Error!")),
    ///         Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
    ///         Ok(Event::Eof) => break,
    ///         _ => (),
    ///     }
    ///     buf.clear();
    /// }
    /// println!("Found {} start events", count);
    /// println!("Text events: {:?}", txt);
    /// ```
    #[inline]
    pub fn read_event_into<'b>(&mut self, buf: &'b mut Vec<u8>) -> Result<Event<'b>> {
        self.read_event_impl(buf)
    }

    /// Reads until end element is found using provided buffer as intermediate
    /// storage for events content. This function is supposed to be called after
    /// you already read a [`Start`] event.
    ///
    /// Manages nested cases where parent and child elements have the same name.
    ///
    /// If corresponding [`End`] event will not be found, the [`Error::UnexpectedEof`]
    /// will be returned. In particularly, that error will be returned if you call
    /// this method without consuming the corresponding [`Start`] event first.
    ///
    /// If your reader created from a string slice or byte array slice, it is
    /// better to use [`read_to_end()`] method, because it will not copy bytes
    /// into intermediate buffer.
    ///
    /// The provided `buf` buffer will be filled only by one event content at time.
    /// Before reading of each event the buffer will be cleared. If you know an
    /// appropriate size of each event, you can preallocate the buffer to reduce
    /// number of reallocations.
    ///
    /// The `end` parameter should contain name of the end element _in the reader
    /// encoding_. It is good practice to always get that parameter using
    /// [`BytesStart::to_end()`] method.
    ///
    /// The correctness of the skipped events does not checked, if you disabled
    /// the [`check_end_names`] option.
    ///
    /// # Namespaces
    ///
    /// While the [`Reader`] does not support namespace resolution, namespaces
    /// does not change the algorithm for comparing names. Although the names
    /// `a:name` and `b:name` where both prefixes `a` and `b` resolves to the
    /// same namespace, are semantically equivalent, `</b:name>` cannot close
    /// `<a:name>`, because according to [the specification]
    ///
    /// > The end of every element that begins with a **start-tag** MUST be marked
    /// > by an **end-tag** containing a name that echoes the element's type as
    /// > given in the **start-tag**
    ///
    /// # Examples
    ///
    /// This example shows, how you can skip XML content after you read the
    /// start event.
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use quick_xml::events::{BytesStart, Event};
    /// use quick_xml::Reader;
    ///
    /// let mut reader = Reader::builder().trim_text(true).into_str_reader(r#"
    ///     <outer>
    ///         <inner>
    ///             <inner></inner>
    ///             <inner/>
    ///             <outer></outer>
    ///             <outer/>
    ///         </inner>
    ///     </outer>
    /// "#);
    /// let mut buf = Vec::new();
    ///
    /// let start = BytesStart::borrowed_name(b"outer");
    /// let end   = start.to_end().into_owned();
    ///
    /// // First, we read a start event...
    /// assert_eq!(reader.read_event_into(&mut buf).unwrap(), Event::Start(start));
    ///
    /// //...then, we could skip all events to the corresponding end event.
    /// // This call will correctly handle nested <outer> elements.
    /// // Note, however, that this method does not handle namespaces.
    /// reader.read_to_end_into(end.name(), &mut buf).unwrap();
    ///
    /// // At the end we should get an Eof event, because we ate the whole XML
    /// assert_eq!(reader.read_event_into(&mut buf).unwrap(), Event::Eof);
    /// ```
    ///
    /// [`Start`]: Event::Start
    /// [`End`]: Event::End
    /// [`read_to_end()`]: Self::read_to_end
    /// [`check_end_names`]: Self::check_end_names
    /// [the specification]: https://www.w3.org/TR/xml11/#dt-etag
    pub fn read_to_end_into(&mut self, end: QName, buf: &mut Vec<u8>) -> Result<()> {
        let mut depth = 0;
        loop {
            buf.clear();
            match self.read_event_into(buf) {
                Err(e) => return Err(e),

                Ok(Event::Start(e)) if e.name() == end => depth += 1,
                Ok(Event::End(e)) if e.name() == end => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Ok(Event::Eof) => {
                    let name = self.decoder().decode(end.as_ref());
                    return Err(Error::UnexpectedEof(format!("</{:?}>", name)));
                }
                _ => (),
            }
        }
    }

    /// Reads optional text between start and end tags.
    ///
    /// If the next event is a [`Text`] event, returns the decoded and unescaped content as a
    /// `String`. If the next event is an [`End`] event, returns the empty string. In all other
    /// cases, returns an error.
    ///
    /// Any text will be decoded using the XML encoding specified in the XML declaration (or UTF-8
    /// if none is specified).
    ///
    /// # Examples
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use quick_xml::Reader;
    /// use quick_xml::events::Event;
    ///
    /// let mut xml = Reader::builder().trim_text(true).into_reader(b"
    ///     <a>&lt;b&gt;</a>
    ///     <a></a>
    /// " as &[u8]);
    ///
    /// let expected = ["<b>", ""];
    /// for &content in expected.iter() {
    ///     match xml.read_event_into(&mut Vec::new()) {
    ///         Ok(Event::Start(ref e)) => {
    ///             assert_eq!(&xml.read_text_into(e.name(), &mut Vec::new()).unwrap(), content);
    ///         },
    ///         e => panic!("Expecting Start event, found {:?}", e),
    ///     }
    /// }
    /// ```
    ///
    /// [`Text`]: Event::Text
    /// [`End`]: Event::End
    pub fn read_text_into(&mut self, end: QName, buf: &mut Vec<u8>) -> Result<String> {
        let s = match self.read_event_into(buf) {
            Err(e) => return Err(e),

            Ok(Event::Text(e)) => e.unescape_and_decode(self),
            Ok(Event::End(e)) if e.name() == end => return Ok("".to_string()),
            Ok(Event::Eof) => return Err(Error::UnexpectedEof("Text".to_string())),
            _ => return Err(Error::TextNotFound),
        };
        self.read_to_end_into(end, buf)?;
        s
    }
}

#[cfg(feature = "async")]
/// Async read methods
impl<R: AsyncBufRead + Unpin + Send, P: Parser + Send> Reader<R, P> {
    /// Reads the next `Event` asynchronously.
    ///
    /// This is the main entry point for reading XML `Event`s.
    ///
    /// `Event`s borrow `buf` and can be converted to own their data if needed (uses `Cow`
    /// internally).
    ///
    /// Having the possibility to control the internal buffers gives you some additional benefits
    /// such as:
    ///
    /// - Reduce the number of allocations by reusing the same buffer. For constrained systems,
    ///   you can call `buf.clear()` once you are done with processing the event (typically at the
    ///   end of your loop).
    /// - Reserve the buffer length if you know the file size (using `Vec::with_capacity`).
    #[inline]
    pub async fn read_event_into_async<'b>(&mut self, buf: &'b mut Vec<u8>) -> Result<Event<'b>>
    where
        R: 'b,
    {
        self.read_event_impl_async(buf).await
    }

    /// Reads asynchronously until end element is found using provided buffer as intermediate
    /// storage for events content. This function is supposed to be called after
    /// you already read a [`Start`] event.
    ///
    /// Manages nested cases where parent and child elements have the same name.
    ///
    /// If corresponding [`End`] event will not be found, the [`Error::UnexpectedEof`]
    /// will be returned. In particularly, that error will be returned if you call
    /// this method without consuming the corresponding [`Start`] event first.
    ///
    /// If your reader created from a string slice or byte array slice, it is
    /// better to use [`read_to_end()`] method, because it will not copy bytes
    /// into intermediate buffer.
    ///
    /// The provided `buf` buffer will be filled only by one event content at time.
    /// Before reading of each event the buffer will be cleared. If you know an
    /// appropriate size of each event, you can preallocate the buffer to reduce
    /// number of reallocations.
    ///
    /// The `end` parameter should contain name of the end element _in the reader
    /// encoding_. It is good practice to always get that parameter using
    /// [`BytesStart::to_end()`] method.
    ///
    /// The correctness of the skipped events does not checked, if you disabled
    /// the [`check_end_names`] option.
    ///
    /// # Namespaces
    ///
    /// While the [`Reader`] does not support namespace resolution, namespaces
    /// does not change the algorithm for comparing names. Although the names
    /// `a:name` and `b:name` where both prefixes `a` and `b` resolves to the
    /// same namespace, are semantically equivalent, `</b:name>` cannot close
    /// `<a:name>`, because according to [the specification]
    ///
    /// > The end of every element that begins with a **start-tag** MUST be marked
    /// > by an **end-tag** containing a name that echoes the element's type as
    /// > given in the **start-tag**
    ///
    /// # Examples
    ///
    /// This example shows, how you can skip XML content after you read the
    /// start event.
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use quick_xml::events::{BytesStart, Event};
    /// use quick_xml::Reader;
    ///
    /// let mut reader = Reader::builder().trim_text(true).into_str_reader(r#"
    ///     <outer>
    ///         <inner>
    ///             <inner></inner>
    ///             <inner/>
    ///             <outer></outer>
    ///             <outer/>
    ///         </inner>
    ///     </outer>
    /// "#);
    /// let mut buf = Vec::new();
    ///
    /// let start = BytesStart::borrowed_name(b"outer");
    /// let end   = start.to_end().into_owned();
    ///
    /// // First, we read a start event...
    /// assert_eq!(reader.read_event_into(&mut buf).unwrap(), Event::Start(start));
    ///
    /// //...then, we could skip all events to the corresponding end event.
    /// // This call will correctly handle nested <outer> elements.
    /// // Note, however, that this method does not handle namespaces.
    /// reader.read_to_end_into(end.name(), &mut buf).unwrap();
    ///
    /// // At the end we should get an Eof event, because we ate the whole XML
    /// assert_eq!(reader.read_event_into(&mut buf).unwrap(), Event::Eof);
    /// ```
    ///
    /// [`Start`]: Event::Start
    /// [`End`]: Event::End
    /// [`read_to_end()`]: Self::read_to_end
    /// [`check_end_names`]: Self::check_end_names
    /// [the specification]: https://www.w3.org/TR/xml11/#dt-etag
    pub async fn read_to_end_into_async(
        &mut self,
        end: QName<'_>,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let mut depth = 0;
        loop {
            buf.clear();
            match self.read_event_into_async(buf).await {
                Err(e) => return Err(e),

                Ok(Event::Start(e)) if e.name() == end => depth += 1,
                Ok(Event::End(e)) if e.name() == end => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Ok(Event::Eof) => {
                    let name = self.decoder().decode(end.as_ref());
                    return Err(Error::UnexpectedEof(format!("</{:?}>", name)));
                }
                _ => (),
            }
        }
    }

    /// Reads optional text between start and end tags.
    ///
    /// If the next event is a [`Text`] event, returns the decoded and unescaped content as a
    /// `String`. If the next event is an [`End`] event, returns the empty string. In all other
    /// cases, returns an error.
    ///
    /// Any text will be decoded using the XML encoding specified in the XML declaration (or UTF-8
    /// if none is specified).
    ///
    /// # Examples
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use quick_xml::Reader;
    /// use quick_xml::events::Event;
    ///
    /// let mut xml = Reader::builder().trim_text(true).into_reader(b"
    ///     <a>&lt;b&gt;</a>
    ///     <a></a>
    /// " as &[u8]);
    ///
    /// let expected = ["<b>", ""];
    /// for &content in expected.iter() {
    ///     match xml.read_event_into(&mut Vec::new()) {
    ///         Ok(Event::Start(ref e)) => {
    ///             assert_eq!(&xml.read_text_into(e.name(), &mut Vec::new()).unwrap(), content);
    ///         },
    ///         e => panic!("Expecting Start event, found {:?}", e),
    ///     }
    /// }
    /// ```
    ///
    /// [`Text`]: Event::Text
    /// [`End`]: Event::End
    pub async fn read_text_into_async(
        &mut self,
        end: QName<'_>,
        buf: &mut Vec<u8>,
    ) -> Result<String> {
        let s = match self.read_event_into_async(buf).await {
            Err(e) => return Err(e),

            Ok(Event::Text(e)) => e.unescape_and_decode(self),
            Ok(Event::End(e)) if e.name() == end => return Ok("".to_string()),
            Ok(Event::Eof) => return Err(Error::UnexpectedEof("Text".to_string())),
            _ => return Err(Error::TextNotFound),
        };
        self.read_to_end_into_async(end, buf).await?;
        s
    }
}

/// Public sync methods for namespaced reader
impl<R: BufRead> Reader<R, NamespacedParser> {
    /// Reads the next event and resolves its namespace (if applicable).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::str::from_utf8;
    /// use quick_xml::Reader;
    /// use quick_xml::events::Event;
    /// use quick_xml::name::ResolveResult::*;
    ///
    /// let xml = r#"<x:tag1 xmlns:x="www.xxxx" xmlns:y="www.yyyy" att1 = "test">
    ///                 <y:tag2><!--Test comment-->Test</y:tag2>
    ///                 <y:tag2>Test 2</y:tag2>
    ///             </x:tag1>"#;
    /// let mut reader =  Reader::builder().trim_text(true).with_namespace().into_str_reader(xml);
    /// let mut count = 0;
    /// let mut buf = Vec::new();
    /// let mut ns_buf = Vec::new();
    /// let mut txt = Vec::new();
    /// loop {
    ///     match reader.read_namespaced_event(&mut buf, &mut ns_buf) {
    ///         Ok((Bound(ns), Event::Start(e))) => {
    ///             count += 1;
    ///             match (ns.as_ref(), e.local_name().as_ref()) {
    ///                 (b"www.xxxx", b"tag1") => (),
    ///                 (b"www.yyyy", b"tag2") => (),
    ///                 (ns, n) => panic!("Namespace and local name mismatch"),
    ///             }
    ///             println!("Resolved namespace: {:?}", ns);
    ///         }
    ///         Ok((Unbound, Event::Start(_))) => {
    ///             panic!("Element not in any namespace")
    ///         },
    ///         Ok((Unknown(p), Event::Start(_))) => {
    ///             panic!("Undeclared namespace prefix {:?}", String::from_utf8(p))
    ///         }
    ///         Ok((_, Event::Text(e))) => {
    ///             txt.push(e.unescape_and_decode(&reader).expect("Error!"))
    ///         },
    ///         Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
    ///         Ok((_, Event::Eof)) => break,
    ///         _ => (),
    ///     }
    ///     buf.clear();
    /// }
    /// println!("Found {} start events", count);
    /// println!("Text events: {:?}", txt);
    /// ```
    pub fn read_namespaced_event<'b, 'ns>(
        &mut self,
        buf: &'b mut Vec<u8>,
        namespace_buffer: &'ns mut Vec<u8>,
    ) -> Result<(ResolveResult<'ns>, Event<'b>)> {
        if self.parser.pending_pop {
            self.parser.ns_resolver.pop(namespace_buffer);
        }
        self.parser.pending_pop = false;
        let event = self.read_event_impl(buf);
        self.read_namespaced_event_internal(event, namespace_buffer)
    }
}

#[cfg(feature = "async")]
/// Public async methods for namespaced reader
impl<R: AsyncBufRead + Unpin + Send> Reader<R, NamespacedParser> {
    /// Reads the next event asynchronously and resolves its namespace (if applicable).
    ///
    /// See also [`Reader::read_namespaced_event`].
    pub async fn read_namespaced_event_async<'b, 'ns>(
        &mut self,
        buf: &'b mut Vec<u8>,
        namespace_buffer: &'ns mut Vec<u8>,
    ) -> Result<(ResolveResult<'ns>, Event<'b>)>
    where
        R: 'b,
    {
        if self.parser.pending_pop {
            self.parser.ns_resolver.pop(namespace_buffer);
        }
        self.parser.pending_pop = false;
        let event = self.read_event_impl_async(buf).await;
        self.read_namespaced_event_internal(event, namespace_buffer)
    }
}

/// Private methods for namespaced parser (no specific reader)
impl<R> Reader<R, NamespacedParser> {
    /// Internal handler for `read_namespaced_event`.
    fn read_namespaced_event_internal<'b, 'ns>(
        &mut self,
        event: Result<Event<'b>>,
        namespace_buffer: &'ns mut Vec<u8>,
    ) -> Result<(ResolveResult<'ns>, Event<'b>)> {
        match event {
            Ok(Event::Eof) => Ok((ResolveResult::Unbound, Event::Eof)),
            Ok(Event::Start(e)) => {
                self.parser.ns_resolver.push(&e, namespace_buffer);
                Ok((
                    self.parser.ns_resolver.find(e.name(), namespace_buffer),
                    Event::Start(e),
                ))
            }
            Ok(Event::Empty(e)) => {
                // For empty elements we need to 'artificially' keep the namespace scope on the
                // stack until the next `next()` call occurs.
                // Otherwise the caller has no chance to use `resolve` in the context of the
                // namespace declarations that are 'in scope' for the empty element alone.
                // Ex: <img rdf:nodeID="abc" xmlns:rdf="urn:the-rdf-uri" />
                self.parser.ns_resolver.push(&e, namespace_buffer);
                // notify next `read_namespaced_event()` invocation that it needs to pop this
                // namespace scope
                self.parser.pending_pop = true;
                Ok((
                    self.parser.ns_resolver.find(e.name(), namespace_buffer),
                    Event::Empty(e),
                ))
            }
            Ok(Event::End(e)) => {
                // notify next `read_namespaced_event()` invocation that it needs to pop this
                // namespace scope
                self.parser.pending_pop = true;
                Ok((
                    self.parser.ns_resolver.find(e.name(), namespace_buffer),
                    Event::End(e),
                ))
            }
            Ok(e) => Ok((ResolveResult::Unbound, e)),
            Err(e) => Err(e),
        }
    }
}

/// Public interface for namespaced parser (no specific reader)
impl<R> Reader<R, NamespacedParser> {
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
        self.parser.event_namespace(name, namespace_buffer)
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
        self.parser.attribute_namespace(name, namespace_buffer)
    }
}

/// Private methods for reading synchronously
impl<R, P: Parser> Reader<R, P> {
    /// Read text into the given buffer, and return an event that borrows from
    /// either that buffer or from the input itself, based on the type of the
    /// reader.
    fn read_event_impl<'i, B>(&mut self, buf: B) -> Result<Event<'i>>
    where
        R: XmlSource<'i, B>,
    {
        let event = match self.parser.tag_state() {
            TagState::Init => self.read_until_open(buf, true),
            TagState::Closed => self.read_until_open(buf, false),
            TagState::Opened => self.read_until_close(buf),
            TagState::Empty => self.parser.close_expanded_empty(),
            TagState::Exit => return Ok(Event::Eof),
        };
        match event {
            Err(_) | Ok(Event::Eof) => self.parser.set_tag_state(TagState::Exit),
            _ => {}
        }
        event
    }

    /// Read until '<' is found and moves reader to an `Opened` state.
    ///
    /// Return a `StartText` event if `first` is `true` and a `Text` event otherwise
    fn read_until_open<'i, B>(&mut self, buf: B, first: bool) -> Result<Event<'i>>
    where
        R: XmlSource<'i, B>,
    {
        self.parser.set_tag_state(TagState::Opened);

        if self.parser.trim_text_start() {
            self.reader
                .skip_whitespace(self.parser.mut_buf_position())?;
        }

        // If we already at the `<` symbol, do not try to return an empty Text event
        if self.reader.skip_one(b'<', self.parser.mut_buf_position())? {
            return self.read_event_impl(buf);
        }

        match self
            .reader
            .read_bytes_until(b'<', buf, self.parser.mut_buf_position())
        {
            Ok(Some(bytes)) => {
                #[cfg(feature = "encoding")]
                if first && self.parser.encoding().can_be_refined() {
                    if let Some(encoding) = detect_encoding(bytes) {
                        self.parser.set_encoding(EncodingRef::BomDetected(encoding));
                    }
                }

                let content = if self.parser.trim_text_end() {
                    // Skip the ending '<
                    let len = bytes
                        .iter()
                        .rposition(|&b| !is_whitespace(b))
                        .map_or_else(|| bytes.len(), |p| p + 1);
                    &bytes[..len]
                } else {
                    bytes
                };

                Ok(if first {
                    Event::StartText(BytesText::from_escaped(content).into())
                } else {
                    Event::Text(BytesText::from_escaped(content))
                })
            }
            Ok(None) => Ok(Event::Eof),
            Err(e) => Err(e),
        }
    }

    /// Private function to read until `>` is found. This function expects that
    /// it was called just after encounter a `<` symbol.
    fn read_until_close<'i, B>(&mut self, buf: B) -> Result<Event<'i>>
    where
        R: XmlSource<'i, B>,
    {
        self.parser.set_tag_state(TagState::Closed);

        match self.reader.peek_one() {
            // `<!` - comment, CDATA or DOCTYPE declaration
            Ok(Some(b'!')) => match self
                .reader
                .read_bang_element(buf, self.parser.mut_buf_position())
            {
                Ok(None) => Ok(Event::Eof),
                Ok(Some((bang_type, bytes))) => self.parser.read_bang(bang_type, bytes),
                Err(e) => Err(e),
            },
            // `</` - closing tag
            Ok(Some(b'/')) => {
                match self
                    .reader
                    .read_bytes_until(b'>', buf, self.parser.mut_buf_position())
                {
                    Ok(None) => Ok(Event::Eof),
                    Ok(Some(bytes)) => self.parser.read_end(bytes),
                    Err(e) => Err(e),
                }
            }
            // `<?` - processing instruction
            Ok(Some(b'?')) => {
                match self
                    .reader
                    .read_bytes_until(b'>', buf, self.parser.mut_buf_position())
                {
                    Ok(None) => Ok(Event::Eof),
                    Ok(Some(bytes)) => self.parser.read_question_mark(bytes),
                    Err(e) => Err(e),
                }
            }
            // `<...` - opening or self-closed tag
            Ok(Some(_)) => match self
                .reader
                .read_element(buf, self.parser.mut_buf_position())
            {
                Ok(None) => Ok(Event::Eof),
                Ok(Some(bytes)) => self.parser.read_start(bytes),
                Err(e) => Err(e),
            },
            Ok(None) => Ok(Event::Eof),
            Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "async")]
/// Private methods for reading asynchronously
impl<R, P: Parser + Send> Reader<R, P> {
    /// Read text into the given buffer, and return an event that borrows from
    /// either that buffer or from the input itself, based on the type of the
    /// reader.
    #[async_recursion::async_recursion]
    async fn read_event_impl_async<'b, B>(&mut self, buf: B) -> Result<Event<'b>>
    where
        R: AsyncXmlSource<'b, B> + Send,
        B: Send,
    {
        let tag_state = self.parser.tag_state();
        let event = match tag_state {
            TagState::Init => self.read_until_open_async(buf, true).await,
            TagState::Closed => self.read_until_open_async(buf, false).await,
            TagState::Opened => self.read_until_close_async(buf).await,
            TagState::Empty => self.parser.close_expanded_empty(),
            TagState::Exit => return Ok(Event::Eof),
        };
        match event {
            Err(_) | Ok(Event::Eof) => self.parser.set_tag_state(TagState::Exit),
            _ => {}
        }
        event
    }

    /// Read until '<' is found and moves reader to an `Opened` state.
    ///
    /// Return a `StartText` event if `first` is `true` and a `Text` event otherwise
    async fn read_until_open_async<'b, B>(&mut self, buf: B, first: bool) -> Result<Event<'b>>
    where
        R: AsyncXmlSource<'b, B> + Send,
        B: Send,
    {
        self.parser.set_tag_state(TagState::Opened);

        if self.parser.trim_text_start() {
            self.reader
                .skip_whitespace(self.parser.mut_buf_position())
                .await?;
        }

        // If we already at the `<` symbol, do not try to return an empty Text event
        if self
            .reader
            .skip_one(b'<', self.parser.mut_buf_position())
            .await?
        {
            return self.read_event_impl_async(buf).await;
        }

        match self
            .reader
            .read_bytes_until(b'<', buf, self.parser.mut_buf_position())
            .await
        {
            Ok(Some(bytes)) => {
                #[cfg(feature = "encoding")]
                if first && self.parser.encoding().can_be_refined() {
                    if let Some(encoding) = detect_encoding(bytes) {
                        self.parser.set_encoding(EncodingRef::BomDetected(encoding));
                    }
                }

                let content = if self.parser.trim_text_end() {
                    // Skip the ending '<
                    let len = bytes
                        .iter()
                        .rposition(|&b| !is_whitespace(b))
                        .map_or_else(|| bytes.len(), |p| p + 1);
                    &bytes[..len]
                } else {
                    bytes
                };

                Ok(if first {
                    Event::StartText(BytesText::from_escaped(content).into())
                } else {
                    Event::Text(BytesText::from_escaped(content))
                })
            }
            Ok(None) => Ok(Event::Eof),
            Err(e) => Err(e),
        }
    }

    /// Private function to read until `>` is found. This function expects that
    /// it was called just after encounter a `<` symbol.
    async fn read_until_close_async<'b, B>(&mut self, buf: B) -> Result<Event<'b>>
    where
        R: AsyncXmlSource<'b, B>,
    {
        self.parser.set_tag_state(TagState::Closed);

        match self.reader.peek_one().await {
            // `<!` - comment, CDATA or DOCTYPE declaration
            Ok(Some(b'!')) => match self
                .reader
                .read_bang_element(buf, self.parser.mut_buf_position())
                .await
            {
                Ok(None) => Ok(Event::Eof),
                Ok(Some((bang_type, bytes))) => self.parser.read_bang(bang_type, bytes),
                Err(e) => Err(e),
            },
            // `</` - closing tag
            Ok(Some(b'/')) => {
                match self
                    .reader
                    .read_bytes_until(b'>', buf, self.parser.mut_buf_position())
                    .await
                {
                    Ok(None) => Ok(Event::Eof),
                    Ok(Some(bytes)) => self.parser.read_end(bytes),
                    Err(e) => Err(e),
                }
            }
            // `<?` - processing instruction
            Ok(Some(b'?')) => {
                match self
                    .reader
                    .read_bytes_until(b'>', buf, self.parser.mut_buf_position())
                    .await
                {
                    Ok(None) => Ok(Event::Eof),
                    Ok(Some(bytes)) => self.parser.read_question_mark(bytes),
                    Err(e) => Err(e),
                }
            }
            // `<...` - opening or self-closed tag
            Ok(Some(_)) => match self
                .reader
                .read_element(buf, self.parser.mut_buf_position())
                .await
            {
                Ok(None) => Ok(Event::Eof),
                Ok(Some(bytes)) => self.parser.read_start(bytes),
                Err(e) => Err(e),
            },
            Ok(None) => Ok(Event::Eof),
            Err(e) => Err(e),
        }
    }
}

impl Reader<BufReader<File>, DefaultParser> {
    /// Creates an XML reader from a file path.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path).map_err(Error::Io)?;
        let reader = BufReader::new(file);
        let parser = DefaultParser::new();
        Ok(Self::from_reader_and_parser(reader, parser))
    }
}

impl<'a> Reader<&'a [u8], DefaultParser> {
    /// Creates an XML reader from a string slice using a default parser configuration.
    pub fn from_str(s: &'a str) -> Self {
        Self::from_str_builder(s, ParserBuilder::<DefaultParser>::new())
    }

    /// Creates an XML reader from a slice of bytes a default parser configuration.
    pub fn from_bytes(s: &'a [u8]) -> Self {
        Self::from_bytes_builder(s, ParserBuilder::<DefaultParser>::new())
    }
}

impl<'a, P: Parser> Reader<&'a [u8], P> {
    /// Creates an XML reader from a string slice and a [`ParserBuilder`] for configuration.
    pub fn from_str_builder(s: &'a str, builder: ParserBuilder<P>) -> Self {
        #[allow(unused_mut)]
        let mut reader = ReaderBuilder::from_parser(builder).into_str_reader(s);

        // Rust strings are guaranteed to be UTF-8, so lock the encoding
        #[cfg(feature = "encoding")]
        {
            reader.parser.set_encoding(EncodingRef::Explicit(UTF_8));
        }

        reader
    }

    /// Creates an XML reader from a slice of bytes a default parser configuration.
    pub fn from_bytes_builder(s: &'a [u8], builder: ParserBuilder<P>) -> Self {
        ReaderBuilder::from_parser(builder).into_reader(s)
    }
}

impl<'a, P: Parser> Reader<&'a [u8], P> {
    /// Read an event that borrows from the input rather than a buffer.
    #[inline]
    pub fn read_event(&mut self) -> Result<Event<'a>> {
        self.read_event_impl(())
    }

    /// Reads until end element is found. This function is supposed to be called
    /// after you already read a [`Start`] event.
    ///
    /// Manages nested cases where parent and child elements have the same name.
    ///
    /// If corresponding [`End`] event will not be found, the [`Error::UnexpectedEof`]
    /// will be returned. In particularly, that error will be returned if you call
    /// this method without consuming the corresponding [`Start`] event first.
    ///
    /// The `end` parameter should contain name of the end element _in the reader
    /// encoding_. It is good practice to always get that parameter using
    /// [`BytesStart::to_end()`] method.
    ///
    /// The correctness of the skipped events does not checked, if you disabled
    /// the [`check_end_names`] option.
    ///
    /// # Namespaces
    ///
    /// While the [`Reader`] does not support namespace resolution, namespaces
    /// does not change the algorithm for comparing names. Although the names
    /// `a:name` and `b:name` where both prefixes `a` and `b` resolves to the
    /// same namespace, are semantically equivalent, `</b:name>` cannot close
    /// `<a:name>`, because according to [the specification]
    ///
    /// > The end of every element that begins with a **start-tag** MUST be marked
    /// > by an **end-tag** containing a name that echoes the element's type as
    /// > given in the **start-tag**
    ///
    /// # Examples
    ///
    /// This example shows, how you can skip XML content after you read the
    /// start event.
    ///
    /// ```
    /// # use pretty_assertions::assert_eq;
    /// use quick_xml::events::{BytesStart, Event};
    /// use quick_xml::Reader;
    ///
    /// let mut reader =  Reader::builder().trim_text(true).into_str_reader(r#"
    ///     <outer>
    ///         <inner>
    ///             <inner></inner>
    ///             <inner/>
    ///             <outer></outer>
    ///             <outer/>
    ///         </inner>
    ///     </outer>
    /// "#);
    ///
    /// let start = BytesStart::borrowed_name(b"outer");
    /// let end   = start.to_end().into_owned();
    ///
    /// // First, we read a start event...
    /// assert_eq!(reader.read_event().unwrap(), Event::Start(start));
    ///
    /// //...then, we could skip all events to the corresponding end event.
    /// // This call will correctly handle nested <outer> elements.
    /// // Note, however, that this method does not handle namespaces.
    /// reader.read_to_end(end.name()).unwrap();
    ///
    /// // At the end we should get an Eof event, because we ate the whole XML
    /// assert_eq!(reader.read_event().unwrap(), Event::Eof);
    /// ```
    ///
    /// [`Start`]: Event::Start
    /// [`End`]: Event::End
    /// [`check_end_names`]: Self::check_end_names
    /// [the specification]: https://www.w3.org/TR/xml11/#dt-etag
    pub fn read_to_end(&mut self, end: QName) -> Result<()> {
        let mut depth = 0;
        loop {
            match self.read_event() {
                Err(e) => return Err(e),

                Ok(Event::Start(e)) if e.name() == end => depth += 1,
                Ok(Event::End(e)) if e.name() == end => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Ok(Event::Eof) => {
                    let name = self.decoder().decode(end.as_ref());
                    return Err(Error::UnexpectedEof(format!("</{:?}>", name)));
                }
                _ => (),
            }
        }
    }
}

/// Possible elements started with `<!`
#[derive(Debug, PartialEq)]
pub enum BangType {
    /// <![CDATA[...]]>
    CData,
    /// <!--...-->
    Comment,
    /// <!DOCTYPE...>
    DocType,
}
impl BangType {
    #[inline(always)]
    fn new(byte: Option<u8>) -> Result<Self> {
        Ok(match byte {
            Some(b'[') => Self::CData,
            Some(b'-') => Self::Comment,
            Some(b'D') | Some(b'd') => Self::DocType,
            Some(b) => return Err(Error::UnexpectedBang(b)),
            None => return Err(Error::UnexpectedEof("Bang".to_string())),
        })
    }

    /// If element is finished, returns its content up to `>` symbol and
    /// an index of this symbol, otherwise returns `None`
    #[inline(always)]
    fn parse<'b>(&self, chunk: &'b [u8], offset: usize) -> Option<(&'b [u8], usize)> {
        for i in memchr::memchr_iter(b'>', chunk) {
            match self {
                // Need to read at least 6 symbols (`!---->`) for properly finished comment
                // <!----> - XML comment
                //  012345 - i
                Self::Comment => {
                    if offset + i > 4 && chunk[..i].ends_with(b"--") {
                        // We cannot strip last `--` from the buffer because we need it in case of
                        // check_comments enabled option. XML standard requires that comment
                        // will not end with `--->` sequence because this is a special case of
                        // `--` in the comment (https://www.w3.org/TR/xml11/#sec-comments)
                        return Some((&chunk[..i], i + 1)); // +1 for `>`
                    }
                }
                Self::CData => {
                    if chunk[..i].ends_with(b"]]") {
                        return Some((&chunk[..i - 2], i + 1)); // +1 for `>`
                    }
                }
                Self::DocType => {
                    let content = &chunk[..i];
                    let balance = memchr::memchr2_iter(b'<', b'>', content)
                        .map(|p| if content[p] == b'<' { 1i32 } else { -1 })
                        .sum::<i32>();
                    if balance == 0 {
                        return Some((content, i + 1)); // +1 for `>`
                    }
                }
            }
        }
        None
    }
    #[inline]
    fn to_err(self) -> Error {
        let bang_str = match self {
            Self::CData => "CData",
            Self::Comment => "Comment",
            Self::DocType => "DOCTYPE",
        };
        Error::UnexpectedEof(bang_str.to_string())
    }
}

/// State machine for the [`XmlSource::read_element`]
#[derive(Clone, Copy)]
enum ReadElementState {
    /// The initial state (inside element, but outside of attribute value)
    Elem,
    /// Inside a single-quoted attribute value
    SingleQ,
    /// Inside a double-quoted attribute value
    DoubleQ,
}
impl ReadElementState {
    /// Changes state by analyzing part of input.
    /// Returns a tuple with part of chunk up to element closing symbol `>`
    /// and a position after that symbol or `None` if such symbol was not found
    #[inline(always)]
    fn change<'b>(&mut self, chunk: &'b [u8]) -> Option<(&'b [u8], usize)> {
        for i in memchr::memchr3_iter(b'>', b'\'', b'"', chunk) {
            *self = match (*self, chunk[i]) {
                // only allowed to match `>` while we are in state `Elem`
                (Self::Elem, b'>') => return Some((&chunk[..i], i + 1)),
                (Self::Elem, b'\'') => Self::SingleQ,
                (Self::Elem, b'\"') => Self::DoubleQ,

                // the only end_byte that gets us out if the same character
                (Self::SingleQ, b'\'') | (Self::DoubleQ, b'"') => Self::Elem,

                // all other bytes: no state change
                _ => *self,
            };
        }
        None
    }
}

/// A function to check whether the byte is a whitespace (blank, new line, carriage return or tab)
#[inline]
pub(crate) fn is_whitespace(b: u8) -> bool {
    match b {
        b' ' | b'\r' | b'\n' | b'\t' => true,
        _ => false,
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////

/// Decoder of byte slices to the strings. This is lightweight object that can be copied.
///
/// If feature `encoding` is enabled, this encoding taken from the `"encoding"`
/// XML declaration or assumes UTF-8, if XML has no <?xml ?> declaration, encoding
/// key is not defined or contains unknown encoding.
///
/// The library supports any UTF-8 compatible encodings that crate `encoding_rs`
/// is supported. [*UTF-16 is not supported at the present*][utf16].
///
/// If feature `encoding` is disabled, the decoder is always UTF-8 decoder:
/// any XML declarations are ignored.
///
/// [utf16]: https://github.com/tafia/quick-xml/issues/158
#[derive(Clone, Copy, Debug)]
pub struct Decoder {
    #[cfg(feature = "encoding")]
    encoding: &'static Encoding,
}

#[cfg(not(feature = "encoding"))]
impl Decoder {
    /// Decodes a UTF8 slice regardless of XML declaration and ignoring BOM if
    /// it is present in the `bytes`.
    ///
    /// Returns an error in case of malformed sequences in the `bytes`.
    ///
    /// If you instead want to use XML declared encoding, use the `encoding` feature
    #[inline]
    pub fn decode<'b>(&self, bytes: &'b [u8]) -> Result<Cow<'b, str>> {
        Ok(Cow::Borrowed(from_utf8(bytes)?))
    }

    /// Decodes a slice regardless of XML declaration with BOM removal if
    /// it is present in the `bytes`.
    ///
    /// Returns an error in case of malformed sequences in the `bytes`.
    ///
    /// If you instead want to use XML declared encoding, use the `encoding` feature
    pub fn decode_with_bom_removal<'b>(&self, bytes: &'b [u8]) -> Result<Cow<'b, str>> {
        let bytes = if bytes.starts_with(b"\xEF\xBB\xBF") {
            &bytes[3..]
        } else {
            bytes
        };
        self.decode(bytes)
    }
}

#[cfg(feature = "encoding")]
impl Decoder {
    /// Returns the `Reader`s encoding.
    ///
    /// This encoding will be used by [`decode`].
    ///
    /// [`decode`]: Self::decode
    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }

    /// Decodes specified bytes using encoding, declared in the XML, if it was
    /// declared there, or UTF-8 otherwise, and ignoring BOM if it is present
    /// in the `bytes`.
    ///
    /// Returns an error in case of malformed sequences in the `bytes`.
    pub fn decode<'b>(&self, bytes: &'b [u8]) -> Result<Cow<'b, str>> {
        match self
            .encoding
            .decode_without_bom_handling_and_without_replacement(bytes)
        {
            None => Err(Error::NonDecodable(None)),
            Some(s) => Ok(s),
        }
    }

    /// Decodes a slice with BOM removal if it is present in the `bytes` using
    /// the reader encoding.
    ///
    /// If this method called after reading XML declaration with the `"encoding"`
    /// key, then this encoding is used, otherwise UTF-8 is used.
    ///
    /// If XML declaration is absent in the XML, UTF-8 is used.
    ///
    /// Returns an error in case of malformed sequences in the `bytes`.
    pub fn decode_with_bom_removal<'b>(&self, bytes: &'b [u8]) -> Result<Cow<'b, str>> {
        self.decode(self.remove_bom(bytes))
    }
    /// Copied from [`Encoding::decode_with_bom_removal`]
    #[inline]
    fn remove_bom<'b>(&self, bytes: &'b [u8]) -> &'b [u8] {
        if self.encoding == UTF_8 && bytes.starts_with(b"\xEF\xBB\xBF") {
            return &bytes[3..];
        }
        if self.encoding == UTF_16LE && bytes.starts_with(b"\xFF\xFE") {
            return &bytes[2..];
        }
        if self.encoding == UTF_16BE && bytes.starts_with(b"\xFE\xFF") {
            return &bytes[2..];
        }

        bytes
    }
}

/// This implementation is required for tests of other parts of the library
#[cfg(test)]
#[cfg(feature = "serialize")]
impl Decoder {
    pub(crate) fn utf8() -> Self {
        Decoder {
            #[cfg(feature = "encoding")]
            encoding: UTF_8,
        }
    }

    #[cfg(feature = "encoding")]
    pub(crate) fn utf16() -> Self {
        Decoder { encoding: UTF_16LE }
    }
}

/// Automatic encoding detection of XML files based using the [recommended algorithm]
/// (https://www.w3.org/TR/xml11/#sec-guessing)
///
/// The algorithm suggests examine up to the first 4 bytes to determine encoding
/// according to the following table:
///
/// | Bytes       |Detected encoding
/// |-------------|------------------------------------------
/// |`00 00 FE FF`|UCS-4, big-endian machine (1234 order)
/// |`FF FE 00 00`|UCS-4, little-endian machine (4321 order)
/// |`00 00 FF FE`|UCS-4, unusual octet order (2143)
/// |`FE FF 00 00`|UCS-4, unusual octet order (3412)
/// |`FE FF ## ##`|UTF-16, big-endian
/// |`FF FE ## ##`|UTF-16, little-endian
/// |`EF BB BF`   |UTF-8
/// |-------------|------------------------------------------
/// |`00 00 00 3C`|UCS-4 or similar (use declared encoding to find the exact one), in big-endian (1234)
/// |`3C 00 00 00`|UCS-4 or similar (use declared encoding to find the exact one), in little-endian (4321)
/// |`00 00 3C 00`|UCS-4 or similar (use declared encoding to find the exact one), in unusual byte orders (2143)
/// |`00 3C 00 00`|UCS-4 or similar (use declared encoding to find the exact one), in unusual byte orders (3412)
/// |`00 3C 00 3F`|UTF-16 BE or ISO-10646-UCS-2 BE or similar 16-bit BE (use declared encoding to find the exact one)
/// |`3C 00 3F 00`|UTF-16 LE or ISO-10646-UCS-2 LE or similar 16-bit LE (use declared encoding to find the exact one)
/// |`3C 3F 78 6D`|UTF-8, ISO 646, ASCII, some part of ISO 8859, Shift-JIS, EUC, or any other 7-bit, 8-bit, or mixed-width encoding which ensures that the characters of ASCII have their normal positions, width, and values; the actual encoding declaration must be read to detect which of these applies, but since all of these encodings use the same bit patterns for the relevant ASCII characters, the encoding declaration itself may be read reliably
/// |`4C 6F A7 94`|EBCDIC (in some flavor; the full encoding declaration must be read to tell which code page is in use)
/// |_Other_      |UTF-8 without an encoding declaration, or else the data stream is mislabeled (lacking a required encoding declaration), corrupt, fragmentary, or enclosed in a wrapper of some kind
///
/// Because [`encoding_rs`] crate supported only subset of those encodings, only
/// supported subset are detected, which is UTF-8, UTF-16 BE and UTF-16 LE.
///
/// If encoding is detected, `Some` is returned, otherwise `None` is returned.
#[cfg(feature = "encoding")]
fn detect_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    match bytes {
        // with BOM
        _ if bytes.starts_with(&[0xFE, 0xFF]) => Some(UTF_16BE),
        _ if bytes.starts_with(&[0xFF, 0xFE]) => Some(UTF_16LE),
        _ if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => Some(UTF_8),

        // without BOM
        _ if bytes.starts_with(&[0x00, b'<', 0x00, b'?']) => Some(UTF_16BE), // Some BE encoding, for example, UTF-16 or ISO-10646-UCS-2
        _ if bytes.starts_with(&[b'<', 0x00, b'?', 0x00]) => Some(UTF_16LE), // Some LE encoding, for example, UTF-16 or ISO-10646-UCS-2
        _ if bytes.starts_with(&[b'<', b'?', b'x', b'm']) => Some(UTF_8), // Some ASCII compatible

        _ => None,
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod test {
    macro_rules! check {
        ($buf:expr) => {
            mod read_bytes_until {
                use crate::reader::XmlSource;
                // Use Bytes for printing bytes as strings for ASCII range
                use crate::utils::Bytes;
                use pretty_assertions::assert_eq;

                /// Checks that search in the empty buffer returns `None`
                #[test]
                fn empty() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"".as_ref();
                    //                ^= 0

                    assert_eq!(
                        input
                            .read_bytes_until(b'*', buf, &mut position)
                            .unwrap()
                            .map(Bytes),
                        None
                    );
                    assert_eq!(position, 0);
                }

                /// Checks that search in the buffer non-existent value returns entire buffer
                /// as a result and set `position` to `len()`
                #[test]
                fn non_existent() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"abcdef".as_ref();
                    //                      ^= 6

                    assert_eq!(
                        input
                            .read_bytes_until(b'*', buf, &mut position)
                            .unwrap()
                            .map(Bytes),
                        Some(Bytes(b"abcdef"))
                    );
                    assert_eq!(position, 6);
                }

                /// Checks that search in the buffer an element that is located in the front of
                /// buffer returns empty slice as a result and set `position` to one symbol
                /// after match (`1`)
                #[test]
                fn at_the_start() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"*abcdef".as_ref();
                    //                 ^= 1

                    assert_eq!(
                        input
                            .read_bytes_until(b'*', buf, &mut position)
                            .unwrap()
                            .map(Bytes),
                        Some(Bytes(b""))
                    );
                    assert_eq!(position, 1); // position after the symbol matched
                }

                /// Checks that search in the buffer an element that is located in the middle of
                /// buffer returns slice before that symbol as a result and set `position` to one
                /// symbol after match
                #[test]
                fn inside() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"abc*def".as_ref();
                    //                    ^= 4

                    assert_eq!(
                        input
                            .read_bytes_until(b'*', buf, &mut position)
                            .unwrap()
                            .map(Bytes),
                        Some(Bytes(b"abc"))
                    );
                    assert_eq!(position, 4); // position after the symbol matched
                }

                /// Checks that search in the buffer an element that is located in the end of
                /// buffer returns slice before that symbol as a result and set `position` to one
                /// symbol after match (`len()`)
                #[test]
                fn in_the_end() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"abcdef*".as_ref();
                    //                       ^= 7

                    assert_eq!(
                        input
                            .read_bytes_until(b'*', buf, &mut position)
                            .unwrap()
                            .map(Bytes),
                        Some(Bytes(b"abcdef"))
                    );
                    assert_eq!(position, 7); // position after the symbol matched
                }
            }

            mod read_bang_element {
                /// Checks that reading CDATA content works correctly
                mod cdata {
                    use crate::errors::Error;
                    use crate::reader::{BangType, XmlSource};
                    use crate::utils::Bytes;
                    use pretty_assertions::assert_eq;

                    /// Checks that if input begins like CDATA element, but CDATA start sequence
                    /// is not finished, parsing ends with an error
                    #[test]
                    #[ignore = "start CDATA sequence fully checked outside of `read_bang_element`"]
                    fn not_properly_start() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"![]]>other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "CData" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("CData")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    /// Checks that if CDATA startup sequence was matched, but an end sequence
                    /// is not found, parsing ends with an error
                    #[test]
                    fn not_closed() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"![CDATA[other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "CData" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("CData")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    /// Checks that CDATA element without content inside parsed successfully
                    #[test]
                    fn empty() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"![CDATA[]]>other content".as_ref();
                        //                           ^= 11

                        assert_eq!(
                            input
                                .read_bang_element(buf, &mut position)
                                .unwrap()
                                .map(|(ty, data)| (ty, Bytes(data))),
                            Some((BangType::CData, Bytes(b"![CDATA[")))
                        );
                        assert_eq!(position, 11);
                    }

                    /// Checks that CDATA element with content parsed successfully.
                    /// Additionally checks that sequences inside CDATA that may look like
                    /// a CDATA end sequence do not interrupt CDATA parsing
                    #[test]
                    fn with_content() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"![CDATA[cdata]] ]>content]]>other content]]>".as_ref();
                        //                                            ^= 28

                        assert_eq!(
                            input
                                .read_bang_element(buf, &mut position)
                                .unwrap()
                                .map(|(ty, data)| (ty, Bytes(data))),
                            Some((BangType::CData, Bytes(b"![CDATA[cdata]] ]>content")))
                        );
                        assert_eq!(position, 28);
                    }
                }

                /// Checks that reading XML comments works correctly. According to the [specification],
                /// comment data can contain any sequence except `--`:
                ///
                /// ```peg
                /// comment = '<--' (!'--' char)* '-->';
                /// char = [#x1-#x2C]
                ///      / [#x2E-#xD7FF]
                ///      / [#xE000-#xFFFD]
                ///      / [#x10000-#x10FFFF]
                /// ```
                ///
                /// The presence of this limitation, however, is simply a poorly designed specification
                /// (maybe for purpose of building of LL(1) XML parser) and quick-xml does not check for
                /// presence of these sequences by default. This tests allow such content.
                ///
                /// [specification]: https://www.w3.org/TR/xml11/#dt-comment
                mod comment {
                    use crate::errors::Error;
                    use crate::reader::{BangType, XmlSource};
                    use crate::utils::Bytes;
                    use pretty_assertions::assert_eq;

                    #[test]
                    #[ignore = "start comment sequence fully checked outside of `read_bang_element`"]
                    fn not_properly_start() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!- -->other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    #[test]
                    fn not_properly_end() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!->other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    #[test]
                    fn not_closed1() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!--other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    #[test]
                    fn not_closed2() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!-->other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    #[test]
                    fn not_closed3() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!--->other content".as_ref();
                        //                ^= 0

                        match input.read_bang_element(buf, &mut position) {
                            Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                            x => assert!(
                                false,
                                r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                                x
                            ),
                        }
                        assert_eq!(position, 0);
                    }

                    #[test]
                    fn empty() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!---->other content".as_ref();
                        //                      ^= 6

                        assert_eq!(
                            input
                                .read_bang_element(buf, &mut position)
                                .unwrap()
                                .map(|(ty, data)| (ty, Bytes(data))),
                            Some((BangType::Comment, Bytes(b"!----")))
                        );
                        assert_eq!(position, 6);
                    }

                    #[test]
                    fn with_content() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"!--->comment<--->other content".as_ref();
                        //                                 ^= 17

                        assert_eq!(
                            input
                                .read_bang_element(buf, &mut position)
                                .unwrap()
                                .map(|(ty, data)| (ty, Bytes(data))),
                            Some((BangType::Comment, Bytes(b"!--->comment<---")))
                        );
                        assert_eq!(position, 17);
                    }
                }

                /// Checks that reading DOCTYPE definition works correctly
                mod doctype {
                    mod uppercase {
                        use crate::errors::Error;
                        use crate::reader::{BangType, XmlSource};
                        use crate::utils::Bytes;
                        use pretty_assertions::assert_eq;

                        #[test]
                        fn not_properly_start() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!D other content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }

                        #[test]
                        fn without_space() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!DOCTYPEother content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }

                        #[test]
                        fn empty() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!DOCTYPE>other content".as_ref();
                            //                         ^= 9

                            assert_eq!(
                                input
                                    .read_bang_element(buf, &mut position)
                                    .unwrap()
                                    .map(|(ty, data)| (ty, Bytes(data))),
                                Some((BangType::DocType, Bytes(b"!DOCTYPE")))
                            );
                            assert_eq!(position, 9);
                        }

                        #[test]
                        fn not_closed() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!DOCTYPE other content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }
                    }

                    mod lowercase {
                        use crate::errors::Error;
                        use crate::reader::{BangType, XmlSource};
                        use crate::utils::Bytes;
                        use pretty_assertions::assert_eq;

                        #[test]
                        fn not_properly_start() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!d other content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }

                        #[test]
                        fn without_space() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!doctypeother content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }

                        #[test]
                        fn empty() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!doctype>other content".as_ref();
                            //                         ^= 9

                            assert_eq!(
                                input
                                    .read_bang_element(buf, &mut position)
                                    .unwrap()
                                    .map(|(ty, data)| (ty, Bytes(data))),
                                Some((BangType::DocType, Bytes(b"!doctype")))
                            );
                            assert_eq!(position, 9);
                        }

                        #[test]
                        fn not_closed() {
                            let buf = $buf;
                            let mut position = 0;
                            let mut input = b"!doctype other content".as_ref();
                            //                ^= 0

                            match input.read_bang_element(buf, &mut position) {
                                Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                                x => assert!(
                                    false,
                                    r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                                    x
                                ),
                            }
                            assert_eq!(position, 0);
                        }
                    }
                }
            }

            mod read_element {
                use crate::reader::XmlSource;
                use crate::utils::Bytes;
                use pretty_assertions::assert_eq;

                /// Checks that nothing was read from empty buffer
                #[test]
                fn empty() {
                    let buf = $buf;
                    let mut position = 0;
                    let mut input = b"".as_ref();
                    //                ^= 0

                    assert_eq!(input.read_element(buf, &mut position).unwrap().map(Bytes), None);
                    assert_eq!(position, 0);
                }

                mod open {
                    use crate::reader::XmlSource;
                    use crate::utils::Bytes;
                    use pretty_assertions::assert_eq;

                    #[test]
                    fn empty_tag() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b">".as_ref();
                        //                 ^= 1

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b""))
                        );
                        assert_eq!(position, 1);
                    }

                    #[test]
                    fn normal() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"tag>".as_ref();
                        //                    ^= 4

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b"tag"))
                        );
                        assert_eq!(position, 4);
                    }

                    #[test]
                    fn empty_ns_empty_tag() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b":>".as_ref();
                        //                  ^= 2

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b":"))
                        );
                        assert_eq!(position, 2);
                    }

                    #[test]
                    fn empty_ns() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b":tag>".as_ref();
                        //                     ^= 5

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b":tag"))
                        );
                        assert_eq!(position, 5);
                    }

                    #[test]
                    fn with_attributes() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = br#"tag  attr-1=">"  attr2  =  '>'  3attr>"#.as_ref();
                        //                                                        ^= 38

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(br#"tag  attr-1=">"  attr2  =  '>'  3attr"#))
                        );
                        assert_eq!(position, 38);
                    }
                }

                mod self_closed {
                    use crate::reader::XmlSource;
                    use crate::utils::Bytes;
                    use pretty_assertions::assert_eq;

                    #[test]
                    fn empty_tag() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"/>".as_ref();
                        //                  ^= 2

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b"/"))
                        );
                        assert_eq!(position, 2);
                    }

                    #[test]
                    fn normal() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b"tag/>".as_ref();
                        //                     ^= 5

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b"tag/"))
                        );
                        assert_eq!(position, 5);
                    }

                    #[test]
                    fn empty_ns_empty_tag() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b":/>".as_ref();
                        //                   ^= 3

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b":/"))
                        );
                        assert_eq!(position, 3);
                    }

                    #[test]
                    fn empty_ns() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = b":tag/>".as_ref();
                        //                      ^= 6

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(b":tag/"))
                        );
                        assert_eq!(position, 6);
                    }

                    #[test]
                    fn with_attributes() {
                        let buf = $buf;
                        let mut position = 0;
                        let mut input = br#"tag  attr-1="/>"  attr2  =  '/>'  3attr/>"#.as_ref();
                        //                                                           ^= 41

                        assert_eq!(
                            input.read_element(buf, &mut position).unwrap().map(Bytes),
                            Some(Bytes(br#"tag  attr-1="/>"  attr2  =  '/>'  3attr/"#))
                        );
                        assert_eq!(position, 41);
                    }
                }
            }

            mod issue_344 {
                use crate::errors::Error;

                #[test]
                fn cdata() {
                    let doc = "![]]>";
                    let mut reader = crate::Reader::from_str(doc);

                    match reader.read_until_close($buf) {
                        Err(Error::UnexpectedEof(s)) if s == "CData" => {}
                        x => assert!(
                            false,
                            r#"Expected `UnexpectedEof("CData")`, but result is: {:?}"#,
                            x
                        ),
                    }
                }

                #[test]
                fn comment() {
                    let doc = "!- -->";
                    let mut reader = crate::Reader::from_str(doc);

                    match reader.read_until_close($buf) {
                        Err(Error::UnexpectedEof(s)) if s == "Comment" => {}
                        x => assert!(
                            false,
                            r#"Expected `UnexpectedEof("Comment")`, but result is: {:?}"#,
                            x
                        ),
                    }
                }

                #[test]
                fn doctype_uppercase() {
                    let doc = "!D>";
                    let mut reader = crate::Reader::from_str(doc);

                    match reader.read_until_close($buf) {
                        Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                        x => assert!(
                            false,
                            r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                            x
                        ),
                    }
                }

                #[test]
                fn doctype_lowercase() {
                    let doc = "!d>";
                    let mut reader = crate::Reader::from_str(doc);

                    match reader.read_until_close($buf) {
                        Err(Error::UnexpectedEof(s)) if s == "DOCTYPE" => {}
                        x => assert!(
                            false,
                            r#"Expected `UnexpectedEof("DOCTYPE")`, but result is: {:?}"#,
                            x
                        ),
                    }
                }
            }

            /// Ensures, that no empty `Text` events are generated
            mod read_event_impl {
                use crate::events::{BytesCData, BytesDecl, BytesEnd, BytesStart, BytesText, Event};
                use crate::reader::Reader;
                use pretty_assertions::assert_eq;

                #[test]
                fn start_text() {
                    let mut reader = Reader::from_str("bom");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::StartText(BytesText::from_escaped(b"bom".as_ref()).into())
                    );
                }

                #[test]
                fn declaration() {
                    let mut reader = Reader::from_str("<?xml ?>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Decl(BytesDecl::from_start(BytesStart::borrowed(b"xml ", 3)))
                    );
                }

                #[test]
                fn doctype() {
                    let mut reader = Reader::from_str("<!DOCTYPE x>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::DocType(BytesText::from_escaped(b"x".as_ref()))
                    );
                }

                #[test]
                fn processing_instruction() {
                    let mut reader = Reader::from_str("<?xml-stylesheet?>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::PI(BytesText::from_escaped(b"xml-stylesheet".as_ref()))
                    );
                }

                #[test]
                fn start() {
                    let mut reader = Reader::from_str("<tag>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Start(BytesStart::borrowed_name(b"tag"))
                    );
                }

                #[test]
                fn end() {
                    // Because we expect invalid XML, do not check that
                    // the end name paired with the start name
                    let mut reader = Reader::builder().check_end_names(false).into_str_reader("</tag>", );

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::End(BytesEnd::borrowed(b"tag"))
                    );
                }

                #[test]
                fn empty() {
                    let mut reader = Reader::from_str("<tag/>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Empty(BytesStart::borrowed_name(b"tag"))
                    );
                }

                /// Text event cannot be generated without preceding event of another type
                #[test]
                fn text() {
                    let mut reader = Reader::from_str("<tag/>text");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Empty(BytesStart::borrowed_name(b"tag"))
                    );

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Text(BytesText::from_escaped(b"text".as_ref()))
                    );
                }

                #[test]
                fn cdata() {
                    let mut reader = Reader::from_str("<![CDATA[]]>");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::CData(BytesCData::from_str(""))
                    );
                }

                #[test]
                fn comment() {
                    let mut reader = Reader::from_str("<!---->");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Comment(BytesText::from_escaped(b"".as_ref()))
                    );
                }

                #[test]
                fn eof() {
                    let mut reader = Reader::from_str("");

                    assert_eq!(
                        reader.read_event_impl($buf).unwrap(),
                        Event::Eof
                    );
                }
            }

            #[cfg(feature = "encoding")]
            mod encoding {
                use crate::events::Event;
                use crate::reader::Reader;
                use encoding_rs::{UTF_8, UTF_16LE, WINDOWS_1251};
                use pretty_assertions::assert_eq;

                mod bytes {
                    use super::*;
                    use pretty_assertions::assert_eq;

                    /// Checks that encoding is detected by BOM and changed after XML declaration
                    #[test]
                    fn bom_detected() {
                        let mut reader = Reader::from_bytes(b"\xFF\xFE<?xml encoding='windows-1251'?>");

                        assert_eq!(reader.decoder().encoding(), UTF_8);
                        reader.read_event_impl($buf).unwrap();
                        assert_eq!(reader.decoder().encoding(), UTF_16LE);

                        reader.read_event_impl($buf).unwrap();
                        assert_eq!(reader.decoder().encoding(), WINDOWS_1251);

                        assert_eq!(reader.read_event_impl($buf).unwrap(), Event::Eof);
                    }

                    /// Checks that encoding is changed by XML declaration, but only once
                    #[test]
                    fn xml_declaration() {
                        let mut reader = Reader::from_bytes(b"<?xml encoding='UTF-16'?><?xml encoding='windows-1251'?>");

                        assert_eq!(reader.decoder().encoding(), UTF_8);
                        reader.read_event_impl($buf).unwrap();
                        assert_eq!(reader.decoder().encoding(), UTF_16LE);

                        reader.read_event_impl($buf).unwrap();
                        assert_eq!(reader.decoder().encoding(), UTF_16LE);

                        assert_eq!(reader.read_event_impl($buf).unwrap(), Event::Eof);
                    }
                }

                /// Checks that XML declaration cannot change the encoding from UTF-8 if
                /// a `Reader` was created using `from_str` method
                #[test]
                fn str_always_has_utf8() {
                    let mut reader = Reader::from_str("<?xml encoding='UTF-16'?>");

                    assert_eq!(reader.decoder().encoding(), UTF_8);
                    reader.read_event_impl($buf).unwrap();
                    assert_eq!(reader.decoder().encoding(), UTF_8);

                    assert_eq!(reader.read_event_impl($buf).unwrap(), Event::Eof);
                }
            }
        };
    }

    /// Tests for reader that generates events that borrow from the provided buffer
    mod buffered {
        check!(&mut Vec::new());
    }

    /// Tests for reader that generates events that borrow from the input
    mod borrowed {
        check!(());
    }
}
