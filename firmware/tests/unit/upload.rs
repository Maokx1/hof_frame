use super::*;
use std::io::Cursor;

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
    for bytes in [
        b"Z\r\n".as_slice(),
        b"3\r\nab",
        b"1\r\naXX",
        b"1\r\na\r\n0\r\n",
    ] {
        assert!(receive(&mut Cursor::new(bytes), BodyLength::Chunked, &mut output).is_err());
    }
    assert!(receive(&mut Cursor::new(b"a"), BodyLength::Known(2), &mut output).is_err());
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
        assert!(metadata.battery_indicator_enabled);
    }
    assert_eq!(
        request(&mut Cursor::new(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")).unwrap(),
        Request::Page
    );
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
        "Transfer-Encoding: gzip\r\n",
        "",
        "Content-Length: -1\r\n",
    ] {
        assert!(request(&mut Cursor::new(headers(extra))).is_err());
    }
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
