use criterion::{self, criterion_group, Criterion};
use pretty_assertions::assert_eq;
use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;

static SAMPLE: &[u8] = include_bytes!("../tests/documents/sample_rss.xml");
static PLAYERS: &[u8] = include_bytes!("../tests/documents/players.xml");

static LOREM_IPSUM_TEXT: &[u8] =
b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt
ut labore et dolore magna aliqua. Hac habitasse platea dictumst vestibulum rhoncus est pellentesque.
Risus ultricies tristique nulla aliquet enim tortor at. Fermentum odio eu feugiat pretium nibh ipsum.
Volutpat sed cras ornare arcu dui. Scelerisque fermentum dui faucibus in ornare quam. Arcu cursus
euismod quis viverra nibh cras pulvinar mattis. Sed viverra tellus in hac habitasse platea. Quis
commodo odio aenean sed. Cursus in hac habitasse platea dictumst quisque sagittis purus.

Neque convallis a cras semper auctor. Sit amet mauris commodo quis imperdiet massa. Ac ut consequat
semper viverra nam libero justo laoreet sit. Adipiscing commodo elit at imperdiet dui accumsan.
Enim lobortis scelerisque fermentum dui faucibus in ornare. Natoque penatibus et magnis dis parturient
montes nascetur ridiculus mus. At lectus urna duis convallis convallis tellus id interdum. Libero
volutpat sed cras ornare arcu dui vivamus arcu. Cursus in hac habitasse platea dictumst quisque sagittis
purus. Consequat id porta nibh venenatis cras sed felis.";

/// Benchmarks the `Reader::read_event` function with all XML well-formless
/// checks disabled (with and without trimming content of #text nodes)
fn read_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_event");
    group.bench_function("trim_text = false", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Start(_)) | Ok(Event::Empty(_)) => count += 1,
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );
        })
    });

    group.bench_function("trim_text = true", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Start(_)) | Ok(Event::Empty(_)) => count += 1,
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );
        });
    });
    group.finish();
}

/// Benchmarks the `Reader::read_namespaced_event` function with all XML well-formless
/// checks disabled (with and without trimming content of #text nodes)
fn read_namespaced_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_namespaced_event");
    group.bench_function("trim_text = false", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            let mut ns_buf = Vec::new();
            loop {
                match r.read_namespaced_event(&mut buf, &mut ns_buf) {
                    Ok((_, Event::Start(_))) | Ok((_, Event::Empty(_))) => count += 1,
                    Ok((_, Event::Eof)) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );
        });
    });

    group.bench_function("trim_text = true", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            let mut ns_buf = Vec::new();
            loop {
                match r.read_namespaced_event(&mut buf, &mut ns_buf) {
                    Ok((_, Event::Start(_))) | Ok((_, Event::Empty(_))) => count += 1,
                    Ok((_, Event::Eof)) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );
        });
    });
    group.finish();
}

/// Benchmarks the `BytesText::unescaped()` method (includes time of `read_event`
/// benchmark)
fn bytes_text_unescaped(c: &mut Criterion) {
    let mut group = c.benchmark_group("BytesText::unescaped");
    group.bench_function("trim_text = false", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut nbtxt = criterion::black_box(0);
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Start(_)) | Ok(Event::Empty(_)) => count += 1,
                    Ok(Event::Text(ref e)) => nbtxt += e.unescaped().unwrap().len(),
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );

            // Windows has \r\n instead of \n
            #[cfg(windows)]
            assert_eq!(
                nbtxt, 67661,
                "Overall length (in bytes) of all text contents of ./tests/documents/sample_rss.xml"
            );

            #[cfg(not(windows))]
            assert_eq!(
                nbtxt, 66277,
                "Overall length (in bytes) of all text contents of ./tests/documents/sample_rss.xml"
            );
        });
    });

    group.bench_function("trim_text = true", |b| {
        b.iter(|| {
            let mut buf = Vec::new();
            let mut r = Reader::from_reader(SAMPLE);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            let mut count = criterion::black_box(0);
            let mut nbtxt = criterion::black_box(0);
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Start(_)) | Ok(Event::Empty(_)) => count += 1,
                    Ok(Event::Text(ref e)) => nbtxt += e.unescaped().unwrap().len(),
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(
                count, 1550,
                "Overall tag count in ./tests/documents/sample_rss.xml"
            );

            // Windows has \r\n instead of \n
            #[cfg(windows)]
            assert_eq!(
                nbtxt, 50334,
                "Overall length (in bytes) of all text contents of ./tests/documents/sample_rss.xml"
            );

            #[cfg(not(windows))]
            assert_eq!(
                nbtxt, 50261,
                "Overall length (in bytes) of all text contents of ./tests/documents/sample_rss.xml"
            );
        });
    });
    group.finish();
}

/// Benchmarks, how fast individual event parsed
fn one_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("One event");
    group.bench_function("StartText", |b| {
        let src = "Hello world!".repeat(512 / 12).into_bytes();
        let mut buf = Vec::with_capacity(1024);
        b.iter(|| {
            let mut r = Reader::from_reader(src.as_ref());
            let mut nbtxt = criterion::black_box(0);
            r.check_end_names(false).check_comments(false);
            match r.read_event_into(&mut buf) {
                Ok(Event::StartText(e)) => nbtxt += e.len(),
                something_else => panic!("Did not expect {:?}", something_else),
            };

            buf.clear();

            assert_eq!(nbtxt, 504);
        })
    });

    group.bench_function("Start", |b| {
        let src = format!(r#"<hello target="{}">"#, "world".repeat(512 / 5)).into_bytes();
        let mut buf = Vec::with_capacity(1024);
        b.iter(|| {
            let mut r = Reader::from_reader(src.as_ref());
            let mut nbtxt = criterion::black_box(0);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            match r.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => nbtxt += e.len(),
                something_else => panic!("Did not expect {:?}", something_else),
            };

            buf.clear();

            assert_eq!(nbtxt, 525);
        })
    });

    group.bench_function("Comment", |b| {
        let src = format!(r#"<!-- hello "{}" -->"#, "world".repeat(512 / 5)).into_bytes();
        let mut buf = Vec::with_capacity(1024);
        b.iter(|| {
            let mut r = Reader::from_reader(src.as_ref());
            let mut nbtxt = criterion::black_box(0);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            match r.read_event_into(&mut buf) {
                Ok(Event::Comment(ref e)) => nbtxt += e.unescaped().unwrap().len(),
                something_else => panic!("Did not expect {:?}", something_else),
            };

            buf.clear();

            assert_eq!(nbtxt, 520);
        })
    });

    group.bench_function("CData", |b| {
        let src = format!(r#"<![CDATA[hello "{}"]]>"#, "world".repeat(512 / 5)).into_bytes();
        let mut buf = Vec::with_capacity(1024);
        b.iter(|| {
            let mut r = Reader::from_reader(src.as_ref());
            let mut nbtxt = criterion::black_box(0);
            r.check_end_names(false)
                .check_comments(false)
                .trim_text(true);
            match r.read_event_into(&mut buf) {
                Ok(Event::CData(ref e)) => nbtxt += e.len(),
                something_else => panic!("Did not expect {:?}", something_else),
            };

            buf.clear();

            assert_eq!(nbtxt, 518);
        })
    });
    group.finish();
}

/// Benchmarks parsing attributes from events
fn attributes(c: &mut Criterion) {
    let mut group = c.benchmark_group("attributes");
    group.bench_function("with_checks = true", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(PLAYERS);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Empty(e)) => {
                        for attr in e.attributes() {
                            let _attr = attr.unwrap();
                            count += 1
                        }
                    }
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(count, 1041);
        })
    });

    group.bench_function("with_checks = false", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(PLAYERS);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Empty(e)) => {
                        for attr in e.attributes().with_checks(false) {
                            let _attr = attr.unwrap();
                            count += 1
                        }
                    }
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(count, 1041);
        })
    });

    group.bench_function("try_get_attribute", |b| {
        b.iter(|| {
            let mut r = Reader::from_reader(PLAYERS);
            r.check_end_names(false).check_comments(false);
            let mut count = criterion::black_box(0);
            let mut buf = Vec::new();
            loop {
                match r.read_event_into(&mut buf) {
                    Ok(Event::Empty(e)) if e.name() == QName(b"player") => {
                        for name in ["num", "status", "avg"] {
                            if let Some(_attr) = e.try_get_attribute(name).unwrap() {
                                count += 1
                            }
                        }
                        assert!(e
                            .try_get_attribute("attribute-that-doesn't-exist")
                            .unwrap()
                            .is_none());
                    }
                    Ok(Event::Eof) => break,
                    _ => (),
                }
                buf.clear();
            }
            assert_eq!(count, 150);
        })
    });
    group.finish();
}

/// Benchmarks escaping text using XML rules
fn escaping(c: &mut Criterion) {
    let mut group = c.benchmark_group("escape_text");

    group.bench_function("no_chars_to_escape_long", |b| {
        b.iter(|| {
            criterion::black_box(escape(LOREM_IPSUM_TEXT));
        })
    });

    group.bench_function("no_chars_to_escape_short", |b| {
        b.iter(|| {
            criterion::black_box(escape(b"just bit of text"));
        })
    });

    group.bench_function("escaped_chars_short", |b| {
        b.iter(|| {
            criterion::black_box(escape(b"age > 72 && age < 21"));
            criterion::black_box(escape(b"\"what's that?\""));
        })
    });

    group.bench_function("escaped_chars_long", |b| {
        let lorem_ipsum_with_escape_chars =
b"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt
ut labore et dolore magna aliqua. & Hac habitasse platea dictumst vestibulum rhoncus est pellentesque.
Risus ultricies tristique nulla aliquet enim tortor at. Fermentum odio eu feugiat pretium nibh ipsum.
Volutpat sed cras ornare arcu dui. Scelerisque fermentum dui faucibus in ornare quam. Arcu cursus
euismod quis< viverra nibh cras pulvinar mattis. Sed viverra tellus in hac habitasse platea. Quis
commodo odio aenean sed. Cursus in hac habitasse platea dictumst quisque sagittis purus.

Neque convallis >a cras semper auctor. Sit amet mauris commodo quis imperdiet massa. Ac ut consequat
semper viverra nam libero justo laoreet sit. 'Adipiscing' commodo elit at imperdiet dui accumsan.
Enim lobortis scelerisque fermentum dui faucibus in ornare. Natoque penatibus et magnis dis parturient
montes nascetur ridiculus mus. At lectus urna duis convallis convallis tellus id interdum. Libero
volutpat sed cras ornare arcu dui vivamus arcu. Cursus in hac habitasse platea dictumst quisque sagittis
purus. Consequat id porta nibh venenatis cras sed felis.";

        b.iter(|| {
            criterion::black_box(escape(lorem_ipsum_with_escape_chars));
        })
    });
    group.finish();
}

/// Benchmarks unescaping text encoded using XML rules
fn unescaping(c: &mut Criterion) {
    let mut group = c.benchmark_group("unescape_text");

    group.bench_function("no_chars_to_unescape_long", |b| {
        b.iter(|| {
            criterion::black_box(unescape(LOREM_IPSUM_TEXT)).unwrap();
        })
    });

    group.bench_function("no_chars_to_unescape_short", |b| {
        b.iter(|| {
            criterion::black_box(unescape(b"just a bit of text")).unwrap();
        })
    });

    group.bench_function("char_reference", |b| {
        b.iter(|| {
            let text = b"prefix &#34;some stuff&#34;,&#x22;more stuff&#x22;";
            criterion::black_box(unescape(text)).unwrap();
            let text = b"&#38;&#60;";
            criterion::black_box(unescape(text)).unwrap();
        })
    });

    group.bench_function("entity_reference", |b| {
        b.iter(|| {
            let text = b"age &gt; 72 &amp;&amp; age &lt; 21";
            criterion::black_box(unescape(text)).unwrap();
            let text = b"&quot;what&apos;s that?&quot;";
            criterion::black_box(unescape(text)).unwrap();
        })
    });

    group.bench_function("mixed", |b| {
        let text =
b"Lorem ipsum dolor sit amet, &amp;consectetur adipiscing elit, sed do eiusmod tempor incididunt
ut labore et dolore magna aliqua. Hac habitasse platea dictumst vestibulum rhoncus est pellentesque.
Risus ultricies &quot;tristique nulla aliquet enim tortor&quot; at. Fermentum odio eu feugiat pretium
nibh ipsum. Volutpat sed cras ornare arcu dui. Scelerisque fermentum dui faucibus in ornare quam. Arcu
cursus euismod quis &#60;viverra nibh cras pulvinar mattis. Sed viverra tellus in hac habitasse platea.
Quis commodo odio aenean sed. Cursus in hac habitasse platea dictumst quisque sagittis purus.

Neque convallis a cras semper auctor. Sit amet mauris commodo quis imperdiet massa. Ac ut consequat
semper viverra nam libero justo &#35; laoreet sit. Adipiscing commodo elit at imperdiet dui accumsan.
Enim lobortis scelerisque fermentum dui faucibus in ornare. Natoque penatibus et magnis dis parturient
montes nascetur ridiculus mus. At lectus urna &#33;duis convallis convallis tellus id interdum. Libero
volutpat sed cras ornare arcu dui vivamus arcu. Cursus in hac habitasse platea dictumst quisque sagittis
purus. Consequat id porta nibh venenatis cras sed felis.";

        b.iter(|| {
            criterion::black_box(unescape(text)).unwrap();
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    read_event,
    bytes_text_unescaped,
    read_namespaced_event,
    one_event,
    attributes,
    escaping,
    unescaping,
);
