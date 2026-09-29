#[cfg(not(feature = "async"))]
mod sync_tests {
    use lopdf::{Dictionary, Document, Error, LoadOptions, Object, ObjectId, Stream};

    /// A one-page PDF whose content stream's `/Length` is the indirect object 5.
    fn pdf_with_an_indirect_length(content: &[u8]) -> Vec<u8> {
        let mut pdf = Vec::new();
        let mut offsets = Vec::new();

        pdf.extend_from_slice(b"%PDF-1.7\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>\nendobj\n",
        );
        offsets.push(pdf.len());
        pdf.extend_from_slice(b"4 0 obj\n<< /Length 5 0 R >>\nstream\n");
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("5 0 obj\n{}\nendobj\n", content.len()).as_bytes());

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
        );
        pdf
    }

    fn stream_ids(document: &Document) -> Vec<ObjectId> {
        document
            .objects
            .iter()
            .filter(|(_, object)| object.as_stream().is_ok())
            .map(|(&id, _)| id)
            .collect()
    }

    #[test]
    fn a_deferred_load_leaves_stream_bytes_in_the_source() {
        let buffer = std::fs::read("assets/example.pdf").unwrap();
        let eager = Document::load_mem(&buffer).unwrap();
        let deferred = Document::load_mem_with_options(&buffer, LoadOptions::with_deferred_stream_content()).unwrap();

        assert_eq!(deferred.objects.len(), eager.objects.len());
        let ids = stream_ids(&deferred);
        assert!(!ids.is_empty());
        for id in ids {
            let stream = deferred.get_object(id).unwrap().as_stream().unwrap();
            assert!(stream.content.is_empty(), "{id:?} was loaded");
            assert!(stream.start_position.is_some(), "{id:?} has no position");
            let expected = &eager.get_object(id).unwrap().as_stream().unwrap().content;
            assert_eq!(&deferred.read_stream_content(&buffer, id).unwrap(), expected, "{id:?}");
        }
    }

    #[test]
    fn an_ordinary_load_can_re_read_its_streams_from_the_source() {
        let buffer = std::fs::read("assets/example.pdf").unwrap();
        let document = Document::load_mem(&buffer).unwrap();

        let ids = stream_ids(&document);
        assert!(!ids.is_empty());
        for id in ids {
            let stream = document.get_object(id).unwrap().as_stream().unwrap();
            assert!(stream.start_position.is_some(), "{id:?} has no position");
            assert_eq!(
                document.read_stream_content(&buffer, id).unwrap(),
                stream.content,
                "{id:?}"
            );
        }
    }

    #[test]
    fn object_streams_are_loaded_even_when_deferred() {
        let buffer = std::fs::read("assets/AnnotationDemo.pdf").unwrap();
        let eager = Document::load_mem(&buffer).unwrap();
        let deferred = Document::load_mem_with_options(&buffer, LoadOptions::with_deferred_stream_content()).unwrap();

        // Every object, including the members of object streams, is present.
        let eager_ids: Vec<_> = eager.objects.keys().collect();
        let deferred_ids: Vec<_> = deferred.objects.keys().collect();
        assert_eq!(deferred_ids, eager_ids);

        // Object streams and the cross-reference stream carry the structure and load
        // regardless; every other stream stays in the source.
        let mut containers = 0;
        for id in stream_ids(&deferred) {
            let stream = deferred.get_object(id).unwrap().as_stream().unwrap();
            if stream.dict.has_type(b"ObjStm") || stream.dict.has_type(b"XRef") {
                assert!(!stream.content.is_empty(), "{id:?} holds structure and must load");
                containers += 1;
            } else {
                assert!(stream.content.is_empty(), "{id:?} was loaded");
            }
        }
        assert!(
            containers > 1,
            "the asset has object streams and a cross-reference stream"
        );
    }

    #[test]
    fn an_indirect_length_is_resolved_on_demand() {
        let content = b"0 0 m 10 10 l S";
        let buffer = pdf_with_an_indirect_length(content);
        let deferred = Document::load_mem_with_options(&buffer, LoadOptions::with_deferred_stream_content()).unwrap();

        let stream = deferred.get_object((4, 0)).unwrap().as_stream().unwrap();
        assert!(stream.content.is_empty());
        assert_eq!(stream.dict.get(b"Length").unwrap().as_reference().unwrap(), (5, 0));
        assert_eq!(deferred.read_stream_content(&buffer, (4, 0)).unwrap(), content);
    }

    #[test]
    fn a_stream_built_in_memory_has_no_source_position() {
        let mut document = Document::new();
        let id = document.add_object(Stream::new(Dictionary::new(), b"built, not parsed".to_vec()));
        assert!(matches!(
            document.read_stream_content(&[], id),
            Err(Error::InvalidStream(message)) if message == "missing start position"
        ));
    }

    #[test]
    fn a_length_past_the_source_is_an_error() {
        let buffer = std::fs::read("assets/example.pdf").unwrap();
        let mut document =
            Document::load_mem_with_options(&buffer, LoadOptions::with_deferred_stream_content()).unwrap();
        let id = stream_ids(&document)[0];
        document
            .get_object_mut(id)
            .and_then(Object::as_stream_mut)
            .unwrap()
            .dict
            .set("Length", buffer.len() as i64);
        assert!(matches!(
            document.read_stream_content(&buffer, id),
            Err(Error::InvalidStream(message)) if message == "stream extends after document end."
        ));
    }

    #[test]
    fn deferring_an_encrypted_document_is_refused() {
        let buffer = std::fs::read("assets/encrypted.pdf").unwrap();
        assert!(matches!(
            Document::load_mem_with_options(&buffer, LoadOptions::with_deferred_stream_content()),
            Err(Error::Unimplemented(_))
        ));
    }

    #[test]
    fn load_options_carry_the_setting() {
        assert!(!LoadOptions::default().defer_stream_content);
        let options = LoadOptions::with_deferred_stream_content();
        assert!(options.defer_stream_content);
        assert!(options.password.is_none());
        assert!(options.filter.is_none());
        assert!(!options.strict);
        assert!(format!("{options:?}").contains("defer_stream_content: true"));
    }
}
