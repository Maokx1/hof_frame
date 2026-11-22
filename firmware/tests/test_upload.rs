use firmware::frame::{Corner, Metadata, Orientation};
use firmware::upload::*;
use std::io::Cursor;
use std::io::{self, Write};
use std::time::Duration;

#[test]
fn progress_response_uses_chunked_framing_and_flushes_each_stage() {
    let mut bytes = Vec::new();
    start_progress(&mut bytes).unwrap();
    let headers = String::from_utf8(bytes.clone()).unwrap();
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(headers.contains("Transfer-Encoding: chunked\r\n"));
    assert!(!headers.contains("Content-Length:"));
    assert!(headers.ends_with("\r\n\r\n"));
    bytes.clear();
    for (stage, text) in [
        (ProgressStage::Received, "received\n"),
        (ProgressStage::Processed, "processed\n"),
        (ProgressStage::Displaying, "displaying\n"),
        (ProgressStage::Displayed, "displayed\n"),
    ] {
        write_progress(&mut bytes, ProgressEvent::Stage(stage)).unwrap();
        assert_eq!(bytes, format!("{:x}\r\n{text}\r\n", text.len()).as_bytes());
        bytes.clear();
    }
}

#[test]
fn final_progress_events_terminate_stream_and_cannot_inject_stages() {
    for (event, line) in [
        (ProgressEvent::Success("Saved."), "success\tSaved.\n"),
        (
            ProgressEvent::Retry(37, "Bad PNG."),
            "retry\t37\tBad PNG.\n",
        ),
        (ProgressEvent::Retry(0, "Expired."), "retry\t0\tExpired.\n"),
        (
            ProgressEvent::Fatal("Fault\r\nsuccess\tfake"),
            "fatal\tFault  success fake\n",
        ),
    ] {
        let mut bytes = Vec::new();
        write_progress(&mut bytes, event).unwrap();
        assert_eq!(
            bytes,
            format!("{:x}\r\n{line}\r\n0\r\n\r\n", line.len()).as_bytes()
        );
    }
}

#[test]
fn progress_write_and_flush_errors_are_explicit() {
    struct FlushFailure(Vec<u8>);
    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "disconnected"))
        }
    }
    assert_eq!(
        start_progress(&mut FlushFailure(Vec::new()))
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe,
    );
    assert_eq!(
        write_progress(
            &mut FlushFailure(Vec::new()),
            ProgressEvent::Stage(ProgressStage::Processed)
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::BrokenPipe,
    );
    let mut bytes = [0_u8; 1];
    assert_eq!(
        write_progress(&mut bytes.as_mut_slice(), ProgressEvent::Success("Saved."))
            .unwrap_err()
            .kind(),
        io::ErrorKind::WriteZero,
    );
}

#[test]
fn final_response_is_complete_and_has_no_retry_window() {
    let body = "Image saved and displayed.";
    let mut bytes = Vec::new();
    write_response(&mut bytes, "200 OK", "text/plain", body, None).unwrap();
    let response = String::from_utf8(bytes).unwrap();
    let (headers, received) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(headers.contains(&format!("Content-Length: {}\r\n", body.len())));
    assert!(headers.contains("Connection: close\r\n"));
    assert!(headers.contains("Cache-Control: no-store\r\n"));
    assert!(!headers.contains("X-Window-Seconds"));
    assert_eq!(received, body);
}

#[test]
fn rejection_response_exposes_remaining_window_including_expiry() {
    for remaining in [0, 1, 123, IDLE_TIMEOUT.as_secs()] {
        let mut bytes = Vec::new();
        write_response(
            &mut bytes,
            "422 Unprocessable Content",
            "text/plain",
            "Choose another image.",
            Some(remaining),
        )
        .unwrap();
        let response = String::from_utf8(bytes).unwrap();
        assert!(response.starts_with("HTTP/1.1 422 Unprocessable Content\r\n"));
        assert!(response.contains(&format!("X-Window-Seconds: {remaining}\r\n")));
        assert!(response.ends_with("\r\n\r\nChoose another image."));
    }
}

#[test]
fn response_write_failure_is_not_reported_as_success() {
    let mut bytes = [0_u8; 4];
    assert_eq!(
        write_response(
            &mut bytes.as_mut_slice(),
            "200 OK",
            "text/plain",
            "saved",
            None
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::WriteZero,
    );
}

#[test]
fn exact_seven_mb_is_accepted_but_one_more_is_rejected() {
    assert_eq!(MAX_UPLOAD_BYTES, 7_000_000);
    let bytes = vec![0x55; MAX_UPLOAD_BYTES];
    let mut output = vec![0; MAX_UPLOAD_BYTES];
    assert_eq!(
        receive(
            &mut Cursor::new(&bytes),
            BodyLength::Known(bytes.len()),
            &mut output
        )
        .unwrap(),
        MAX_UPLOAD_BYTES
    );
    assert_eq!(output, bytes);
    assert!(matches!(
        receive(
            &mut Cursor::new([]),
            BodyLength::Known(MAX_UPLOAD_BYTES + 1),
            &mut output
        ),
        Err(ReceiveError::TooLarge)
    ));
}

#[test]
fn streams_seven_mb_in_small_writes_and_reports_storage_failures() {
    struct Sink {
        written: usize,
        fail: bool,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(bytes.len() <= 4096);
            if self.fail {
                return Err(io::Error::other("flash write failed"));
            }
            self.written += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink {
        written: 0,
        fail: false,
    };
    for body in [BodyLength::Known(MAX_UPLOAD_BYTES), BodyLength::Chunked] {
        let bytes = vec![0x55; MAX_UPLOAD_BYTES];
        let mut encoded = Vec::new();
        if body == BodyLength::Chunked {
            for chunk in bytes.chunks(3001) {
                write!(encoded, "{:x}\r\n", chunk.len()).unwrap();
                encoded.extend_from_slice(chunk);
                encoded.extend_from_slice(b"\r\n");
            }
            encoded.extend_from_slice(b"0\r\n\r\n");
        } else {
            encoded = bytes;
        }
        sink.written = 0;
        assert_eq!(
            receive_to(&mut Cursor::new(encoded), body, &mut sink, usize::MAX).unwrap(),
            7_000_000
        );
        assert_eq!(sink.written, 7_000_000);
    }
    sink.fail = true;
    assert!(matches!(
        receive_to(
            &mut Cursor::new(b"x"),
            BodyLength::Known(1),
            &mut sink,
            MAX_UPLOAD_BYTES
        ),
        Err(ReceiveError::Storage(_))
    ));
    sink.fail = false;
    sink.written = 0;
    assert!(matches!(
        receive_to(
            &mut Cursor::new([]),
            BodyLength::Known(7_000_001),
            &mut sink,
            usize::MAX
        ),
        Err(ReceiveError::TooLarge)
    ));
    assert_eq!(sink.written, 0);
}

#[test]
fn upload_read_failures_are_distinguished_from_storage_failures() {
    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "client disconnected",
            ))
        }
    }

    let mut reader = io::BufReader::new(FailingReader);
    let mut destination = Vec::new();
    let result = receive_to(&mut reader, BodyLength::Known(1), &mut destination, 1);
    assert!(matches!(
        result,
        Err(ReceiveError::Io(error)) if error.kind() == io::ErrorKind::ConnectionReset
    ));
    assert!(destination.is_empty());
}

#[test]
fn declared_seven_mb_is_accepted_and_one_more_is_rejected_before_writing() {
    for (length, accepted) in [(7_000_000, true), (7_000_001, false)] {
        let result = request(&mut Cursor::new(headers(&format!(
            "Content-Length: {length}\r\n"
        ))));
        if accepted {
            assert!(matches!(
                result,
                Ok(Request::Upload {
                    body: BodyLength::Known(7_000_000),
                    ..
                })
            ));
        } else {
            assert!(matches!(result, Err(ReceiveError::TooLarge)));
        }
    }
}

#[test]
fn chunked_unknown_length_is_bounded_cumulatively() {
    let mut output = [0; 5];
    assert_eq!(
        receive(
            &mut Cursor::new(b"2\r\nab\r\n3;x=y\r\ncde\r\n0\r\n\r\n"),
            BodyLength::Chunked,
            &mut output
        )
        .unwrap(),
        5
    );
    assert_eq!(&output, b"abcde");
    assert!(matches!(
        receive(
            &mut Cursor::new(b"3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n"),
            BodyLength::Chunked,
            &mut output
        ),
        Err(ReceiveError::TooLarge)
    ));
}

#[test]
fn malformed_or_truncated_bodies_fail() {
    let mut output = [0; 20];
    for bytes in [b"Z\r\n".as_slice(), b"1\r\naXX", b"1\r\na\r\n0\r\n"] {
        assert!(matches!(
            receive(&mut Cursor::new(bytes), BodyLength::Chunked, &mut output),
            Err(ReceiveError::InvalidRequest)
        ));
    }
    for (bytes, length) in [
        (b"3\r\nab".as_slice(), BodyLength::Chunked),
        (b"a".as_slice(), BodyLength::Known(2)),
    ] {
        assert!(matches!(
            receive(&mut Cursor::new(bytes), length, &mut output),
            Err(ReceiveError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
    }
}

#[test]
fn chunk_framing_limits_and_size_overflow_fail_without_writing() {
    let oversized_chunk_header = format!("1;{}\r\n", "x".repeat(128 * 1024));
    let oversized_trailer = format!("0\r\nX: {}\r\n\r\n", "x".repeat(8192));
    for body in [oversized_chunk_header, oversized_trailer] {
        let mut destination = Vec::new();
        assert!(matches!(
            receive_to(
                &mut Cursor::new(body),
                BodyLength::Chunked,
                &mut destination,
                MAX_UPLOAD_BYTES,
            ),
            Err(ReceiveError::InvalidRequest)
        ));
        assert!(destination.is_empty());
    }

    let overflow = format!("{}\r\n", "F".repeat(2 * std::mem::size_of::<usize>() + 1));
    let mut destination = Vec::new();
    assert!(matches!(
        receive_to(
            &mut Cursor::new(overflow),
            BodyLength::Chunked,
            &mut destination,
            MAX_UPLOAD_BYTES,
        ),
        Err(ReceiveError::TooLarge)
    ));
    assert!(destination.is_empty());
}

fn headers(extra: &str) -> Vec<u8> {
    headers_with_query("orientation=landscape&corner=bottom-left", extra)
}

fn headers_with_query(query: &str, extra: &str) -> Vec<u8> {
    format!("POST /upload?{query} HTTP/1.1\r\nContent-Type: image/png\r\n{extra}\r\n").into_bytes()
}

#[test]
fn parses_layout_and_chunked_or_declared_length() {
    for extra in [
        "Content-Length: 4194304\r\n",
        "Transfer-Encoding: chunked\r\n",
    ] {
        let Request::Upload { metadata, .. } = request(&mut Cursor::new(headers(extra))).unwrap()
        else {
            panic!("upload required")
        };
        assert_eq!(metadata.orientation, Orientation::Landscape);
        assert_eq!(metadata.battery_corner, Corner::BottomLeft);
        assert!(!metadata.battery_indicator_enabled);
    }
    assert_eq!(
        request(&mut Cursor::new(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")).unwrap(),
        Request::Page
    );
}

#[test]
fn all_upload_layouts_and_indicator_settings_are_accepted() {
    for (orientation, expected_orientation) in [
        ("portrait", Orientation::Portrait),
        ("landscape", Orientation::Landscape),
    ] {
        for (corner, expected_corner) in [
            ("top-right", Corner::TopRight),
            ("top-left", Corner::TopLeft),
            ("bottom-left", Corner::BottomLeft),
            ("bottom-right", Corner::BottomRight),
        ] {
            for (indicator, enabled) in [("on", true), ("off", false)] {
                let query =
                    format!("orientation={orientation}&corner={corner}&indicator={indicator}");
                let request = request(&mut Cursor::new(headers_with_query(
                    &query,
                    "Content-Length: 1\r\n",
                )))
                .unwrap();
                assert_eq!(
                    request,
                    Request::Upload {
                        body: BodyLength::Known(1),
                        metadata: Metadata {
                            orientation: expected_orientation,
                            battery_corner: expected_corner,
                            battery_indicator_enabled: enabled,
                        },
                    }
                );
            }
        }
    }
}

#[test]
fn malformed_upload_layouts_are_rejected() {
    for query in [
        "orientation=sideways&corner=top-right",
        "orientation=landscape&corner=center",
        "orientation=portrait",
        "corner=top-right",
        "orientation&corner=top-right",
        "orientation=portrait&corner=top-right&corner=bottom-left",
        "orientation=portrait&corner=top-right&orientation=landscape",
        "orientation=portrait&corner=top-right&extra=value",
    ] {
        assert!(
            matches!(
                request(&mut Cursor::new(headers_with_query(
                    query,
                    "Content-Length: 1\r\n",
                ))),
                Err(ReceiveError::InvalidRequest)
            ),
            "{query}"
        );
    }
}

#[test]
fn invalid_request_lines_and_headers_are_rejected() {
    for request_bytes in [
        b"GET / HTTP/1.1\n\n".as_slice(),
        b"GET / HTTP/1.0\r\n\r\n",
        b"GET /\r\n\r\n",
        b"GET / HTTP/1.1 extra\r\n\r\n",
        b"GET / HTTP/1.1\r\nX: \xff\r\n\r\n",
        b"GET / HTTP/1.1\r\nMalformed\r\n\r\n",
    ] {
        assert!(
            matches!(
                request(&mut Cursor::new(request_bytes)),
                Err(ReceiveError::InvalidRequest)
            ),
            "{request_bytes:?}"
        );
    }
}

#[test]
fn request_header_budget_accepts_exact_limit_and_rejects_one_more_byte() {
    let prefix = "GET / HTTP/1.1\r\nX: ";
    let suffix = "\r\n\r\n";
    let padding = 8192 - prefix.len() - suffix.len();
    let exact = format!("{prefix}{}{suffix}", "x".repeat(padding));
    assert_eq!(request(&mut Cursor::new(exact)).unwrap(), Request::Page);

    let oversized = format!("{prefix}{}{suffix}", "x".repeat(padding + 1));
    assert!(matches!(
        request(&mut Cursor::new(oversized)),
        Err(ReceiveError::InvalidRequest)
    ));
}

#[test]
fn unsupported_routes_methods_and_image_types_are_rejected() {
    for request_bytes in [
        "DELETE / HTTP/1.1\r\n\r\n",
        "GET / HTTP/1.1\r\nContent-Length: 1\r\n\r\n",
        "POST /upload HTTP/1.1\r\nContent-Type: image/png\r\nContent-Length: 1\r\n\r\n",
        concat!(
            "POST /other?orientation=landscape&corner=top-right HTTP/1.1\r\n",
            "Content-Type: image/png\r\nContent-Length: 1\r\n\r\n",
        ),
    ] {
        assert!(
            matches!(
                request(&mut Cursor::new(request_bytes)),
                Err(ReceiveError::InvalidRequest)
            ),
            "{request_bytes:?}"
        );
    }

    for content_type in ["", "Content-Type: image/gif\r\n"] {
        let bytes = format!(
            "POST /upload?orientation=landscape&corner=top-right HTTP/1.1\r\n{content_type}Content-Length: 1\r\n\r\n"
        );
        assert!(matches!(
            request(&mut Cursor::new(bytes)),
            Err(ReceiveError::UnsupportedImage)
        ));
    }
}

#[test]
fn indicator_setting_can_be_disabled_and_rejects_invalid_values() {
    let Request::Upload { metadata, .. } = request(&mut Cursor::new(headers_with_query(
        "orientation=landscape&corner=bottom-left&indicator=off",
        "Content-Length: 1\r\n",
    )))
    .unwrap() else {
        panic!("upload required")
    };
    assert!(!metadata.battery_indicator_enabled);

    let Request::Upload { metadata, .. } = request(&mut Cursor::new(headers_with_query(
        "orientation=landscape&corner=bottom-left&indicator=on",
        "Content-Length: 1\r\n",
    )))
    .unwrap() else {
        panic!("upload required")
    };
    assert!(metadata.battery_indicator_enabled);

    for query in [
        "orientation=landscape&corner=bottom-left&indicator=maybe",
        "orientation=landscape&corner=bottom-left&indicator=off&indicator=on",
    ] {
        assert!(request(&mut Cursor::new(headers_with_query(
            query,
            "Content-Length: 1\r\n",
        )))
        .is_err());
    }
}

#[test]
fn rejects_oversize_duplicate_and_ambiguous_framing() {
    assert!(matches!(
        request(&mut Cursor::new(headers("Content-Length: 7000001\r\n"))),
        Err(ReceiveError::TooLarge)
    ));
    for extra in [
        "Content-Length: 1\r\nContent-Length: 1\r\n",
        "Content-Length: 1\r\nTransfer-Encoding: chunked\r\n",
        "Content-Length: 1\r\nContent-Type: image/jpeg\r\n",
        "Content-Length: 1\r\nExpect: 100-continue\r\n",
        "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n",
        "Transfer-Encoding: gzip\r\n",
        "",
        "Content-Length: -1\r\n",
    ] {
        assert!(
            matches!(
                request(&mut Cursor::new(headers(extra))),
                Err(ReceiveError::InvalidRequest)
            ),
            "{extra:?}"
        );
    }
    let overflow = format!("Content-Length: {}0\r\n", usize::MAX);
    assert!(matches!(
        request(&mut Cursor::new(headers(&overflow))),
        Err(ReceiveError::TooLarge)
    ));
    assert!(request(&mut Cursor::new(format!(
        "GET / HTTP/1.1\r\nX: {}\r\n\r\n",
        "a".repeat(9000)
    )))
    .is_err());
}

#[test]
fn signature_checks_are_not_decode_validation() {
    assert!(image_signature(b"\x89PNG\r\n\x1a\n"));
    assert!(image_signature(&[0xFF, 0xD8, 0xFF]));
    assert!(!image_signature(b"not an image"));
}

#[test]
fn absolute_deadline_stops_slow_drip_despite_recent_data() {
    let start = std::time::Instant::now();
    let deadline = start + TRANSFER_TIMEOUT;
    assert!(!receive_timed_out(
        start + Duration::from_secs(119),
        deadline,
        start + Duration::from_secs(118),
        INACTIVITY_TIMEOUT
    ));
    assert!(receive_timed_out(
        start + Duration::from_secs(120),
        deadline,
        start + Duration::from_secs(119),
        INACTIVITY_TIMEOUT
    ));
}

#[test]
fn inactivity_and_header_timeouts_include_exact_threshold() {
    let start = std::time::Instant::now();
    let deadline = start + AWAKE_TIMEOUT;
    assert!(!receive_timed_out(
        start + Duration::from_secs(119),
        deadline,
        start,
        INACTIVITY_TIMEOUT
    ));
    assert!(receive_timed_out(
        start + Duration::from_secs(120),
        deadline,
        start,
        INACTIVITY_TIMEOUT
    ));
    assert!(receive_timed_out(
        start + HEADER_TIMEOUT,
        start + HEADER_TIMEOUT,
        start,
        HEADER_TIMEOUT
    ));
    assert_eq!(IDLE_TIMEOUT.as_secs(), 300);
    assert_eq!(AWAKE_TIMEOUT.as_secs(), 600);
}

#[test]
fn exact_limit_chunked_body_passes_and_next_byte_fails() {
    let mut body = format!("{:x}\r\n", MAX_UPLOAD_BYTES).into_bytes();
    body.extend(vec![0; MAX_UPLOAD_BYTES]);
    body.extend(b"\r\n0\r\n\r\n");
    let mut output = vec![0; MAX_UPLOAD_BYTES];
    assert_eq!(
        receive(&mut Cursor::new(&body), BodyLength::Chunked, &mut output).unwrap(),
        MAX_UPLOAD_BYTES
    );
    let end = body.len() - b"0\r\n\r\n".len();
    body.truncate(end);
    body.extend(b"1\r\nx\r\n0\r\n\r\n");
    assert!(matches!(
        receive(&mut Cursor::new(&body), BodyLength::Chunked, &mut output),
        Err(ReceiveError::TooLarge)
    ));
}

#[test]
fn arbitrarily_fragmented_request_and_body_are_parsed() {
    let mut data = headers("Transfer-Encoding: chunked\r\n");
    data.extend(b"3\r\nabc\r\n0\r\n\r\n");
    let mut reader = std::io::BufReader::with_capacity(1, Cursor::new(data));
    assert!(matches!(
        request(&mut reader),
        Ok(Request::Upload {
            body: BodyLength::Chunked,
            ..
        })
    ));
    let mut output = [0; 3];
    assert_eq!(
        receive(&mut reader, BodyLength::Chunked, &mut output).unwrap(),
        3
    );
    assert_eq!(&output, b"abc");
}

#[test]
fn favicon_does_not_become_an_upload_attempt() {
    assert_eq!(
        request(&mut Cursor::new(b"GET /favicon.ico HTTP/1.1\r\n\r\n")).unwrap(),
        Request::NotFound
    );
}

#[test]
fn wifi_password_requires_eight_to_sixty_three_printable_ascii_bytes() {
    for length in [0, 7, 64] {
        assert!(!valid_wifi_password(&"x".repeat(length)));
    }
    for length in [8, 63] {
        assert!(valid_wifi_password(&"x".repeat(length)));
    }
    assert!(!valid_wifi_password("abcd\nefgh"));
    assert!(!valid_wifi_password("abcdefg\u{e9}"));
}
