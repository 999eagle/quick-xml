use quick_xml::{events::Event::*, Reader};
use std::path::PathBuf;
use std::str::from_utf8;

use pretty_assertions::assert_eq;

#[tokio::test]
async fn test_sample() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/documents/sample_rss.xml");
    let file = tokio::fs::File::open(path).await.unwrap();
    let file = tokio::io::BufReader::new(file);
    let mut buf = Vec::new();
    let mut r = Reader::from_reader(file);
    let mut count = 0;
    loop {
        match r.read_event_into_async(&mut buf).await.unwrap() {
            Start(_) => count += 1,
            Decl(e) => println!("{:?}", e.version()),
            Eof => break,
            _ => (),
        }
        buf.clear();
    }
    println!("{}", count);
}

#[tokio::test]
async fn test_xml_decl() {
    let mut r = Reader::builder()
        .trim_text(true)
        .into_str_reader("<?xml version=\"1.0\" encoding='utf-8'?>");
    let mut buf = Vec::new();
    match r.read_event_into_async(&mut buf).await.unwrap() {
        Decl(ref e) => {
            match e.version() {
                Ok(v) => assert_eq!(
                    &*v,
                    b"1.0",
                    "expecting version '1.0', got '{:?}",
                    from_utf8(&*v)
                ),
                Err(e) => assert!(false, "{:?}", e),
            }
            match e.encoding() {
                Some(Ok(v)) => assert_eq!(
                    &*v,
                    b"utf-8",
                    "expecting encoding 'utf-8', got '{:?}",
                    from_utf8(&*v)
                ),
                Some(Err(e)) => panic!("{:?}", e),
                None => panic!("cannot find encoding"),
            }
            match e.standalone() {
                None => (),
                e => panic!("doesn't expect standalone, got {:?}", e),
            }
        }
        _ => panic!("unable to parse XmlDecl"),
    }
}
