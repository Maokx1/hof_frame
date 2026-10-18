use std::io::{self, BufRead};
use std::time::Duration;

use crate::frame::{Corner, Metadata, Orientation};

pub const MAX_UPLOAD_BYTES: usize = 4_194_304;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
pub const AWAKE_TIMEOUT: Duration = Duration::from_secs(600);
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEADER_BYTES: usize = 8192;

pub fn valid_wifi_password(password: &str) -> bool {
    (8..=63).contains(&password.len()) && password.bytes().all(|byte| (0x20..=0x7E).contains(&byte))
}

pub fn receive_timed_out(
    now: std::time::Instant,
    deadline: std::time::Instant,
    last_data: std::time::Instant,
    inactivity: Duration,
) -> bool {
    now >= deadline || now.duration_since(last_data) >= inactivity
}

#[derive(Debug)]
pub enum ReceiveError {
    Io(io::Error),
    InvalidRequest,
    TooLarge,
    UnsupportedImage,
}

impl From<io::Error> for ReceiveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyLength {
    Known(usize),
    Chunked,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Page,
    NotFound,
    Upload {
        body: BodyLength,
        metadata: Metadata,
    },
}

fn line(reader: &mut impl BufRead, remaining: &mut usize) -> Result<String, ReceiveError> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Err(ReceiveError::InvalidRequest);
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |pos| pos + 1);
        if count > *remaining {
            return Err(ReceiveError::InvalidRequest);
        }
        let done = buffer[count - 1] == b'\n';
        bytes.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        *remaining -= count;
        if done {
            break;
        }
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(ReceiveError::InvalidRequest);
    }
    bytes.truncate(bytes.len() - 2);
    String::from_utf8(bytes).map_err(|_| ReceiveError::InvalidRequest)
}

pub fn request(reader: &mut impl BufRead) -> Result<Request, ReceiveError> {
    let mut remaining = MAX_HEADER_BYTES;
    let first = line(reader, &mut remaining)?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" {
        return Err(ReceiveError::InvalidRequest);
    }
    let mut length = None;
    let mut chunked = false;
    let mut content_type = None;
    let mut expect_continue = false;
    loop {
        let header = line(reader, &mut remaining)?;
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(ReceiveError::InvalidRequest)?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
                || value.is_empty()
            {
                return Err(ReceiveError::InvalidRequest);
            }
            length = Some(value.parse::<usize>().map_err(|_| ReceiveError::TooLarge)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.eq_ignore_ascii_case("chunked") {
                return Err(ReceiveError::InvalidRequest);
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-type") {
            if content_type.is_some() {
                return Err(ReceiveError::InvalidRequest);
            }
            content_type = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("expect") {
            expect_continue = true;
        }
    }
    if expect_continue || (chunked && length.is_some()) {
        return Err(ReceiveError::InvalidRequest);
    }
    if length.is_some_and(|len| len > MAX_UPLOAD_BYTES) {
        return Err(ReceiveError::TooLarge);
    }
    if parts[0] == "GET" && !chunked && length.unwrap_or(0) == 0 {
        return Ok(if parts[1] == "/" {
            Request::Page
        } else {
            Request::NotFound
        });
    }
    if parts[0] != "POST" {
        return Err(ReceiveError::InvalidRequest);
    }
    let (path, query) = parts[1]
        .split_once('?')
        .ok_or(ReceiveError::InvalidRequest)?;
    if path != "/upload" {
        return Err(ReceiveError::InvalidRequest);
    }
    if !matches!(content_type.as_deref(), Some("image/png" | "image/jpeg")) {
        return Err(ReceiveError::UnsupportedImage);
    }
    let mut orientation = None;
    let mut corner = None;
    for parameter in query.split('&') {
        let (name, value) = parameter
            .split_once('=')
            .ok_or(ReceiveError::InvalidRequest)?;
        match (name, value) {
            ("orientation", value) if orientation.is_none() => {
                orientation = Some(match value {
                    "portrait" => Orientation::Portrait,
                    "landscape" => Orientation::Landscape,
                    _ => return Err(ReceiveError::InvalidRequest),
                })
            }
            ("corner", value) if corner.is_none() => {
                corner = Some(match value {
                    "top-right" => Corner::TopRight,
                    "top-left" => Corner::TopLeft,
                    "bottom-left" => Corner::BottomLeft,
                    "bottom-right" => Corner::BottomRight,
                    _ => return Err(ReceiveError::InvalidRequest),
                })
            }
            _ => return Err(ReceiveError::InvalidRequest),
        }
    }
    Ok(Request::Upload {
        body: if chunked {
            BodyLength::Chunked
        } else {
            BodyLength::Known(length.ok_or(ReceiveError::InvalidRequest)?)
        },
        metadata: Metadata {
            orientation: orientation.ok_or(ReceiveError::InvalidRequest)?,
            battery_corner: corner.ok_or(ReceiveError::InvalidRequest)?,
        },
    })
}

pub fn receive(
    reader: &mut impl BufRead,
    body: BodyLength,
    destination: &mut [u8],
) -> Result<usize, ReceiveError> {
    let capacity = destination.len().min(MAX_UPLOAD_BYTES);
    match body {
        BodyLength::Known(len) => {
            if len > capacity {
                return Err(ReceiveError::TooLarge);
            }
            reader.read_exact(&mut destination[..len])?;
            Ok(len)
        }
        BodyLength::Chunked => {
            let mut total = 0;
            let mut framing_budget = 128 * 1024;
            loop {
                let header = line(reader, &mut framing_budget)?;
                let size = header
                    .split(';')
                    .next()
                    .ok_or(ReceiveError::InvalidRequest)?;
                if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(ReceiveError::InvalidRequest);
                }
                let size = usize::from_str_radix(size, 16).map_err(|_| ReceiveError::TooLarge)?;
                if size == 0 {
                    let mut trailers = MAX_HEADER_BYTES;
                    while !line(reader, &mut trailers)?.is_empty() {}
                    return Ok(total);
                }
                let end = total
                    .checked_add(size)
                    .filter(|end| *end <= capacity)
                    .ok_or(ReceiveError::TooLarge)?;
                reader.read_exact(&mut destination[total..end])?;
                let mut crlf = [0; 2];
                reader.read_exact(&mut crlf)?;
                if crlf != *b"\r\n" {
                    return Err(ReceiveError::InvalidRequest);
                }
                total = end;
            }
        }
    }
}

pub fn image_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn exact_four_mib_is_accepted_but_one_more_is_rejected() {
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
        format!("POST /upload?orientation=landscape&corner=bottom-left HTTP/1.1\r\nContent-Type: image/png\r\n{extra}\r\n").into_bytes()
    }

    #[test]
    fn parses_layout_and_chunked_or_declared_length() {
        for extra in [
            "Content-Length: 4194304\r\n",
            "Transfer-Encoding: chunked\r\n",
        ] {
            let Request::Upload { metadata, .. } =
                request(&mut Cursor::new(headers(extra))).unwrap()
            else {
                panic!("upload required")
            };
            assert_eq!(metadata.orientation, Orientation::Landscape);
            assert_eq!(metadata.battery_corner, Corner::BottomLeft);
        }
        assert_eq!(
            request(&mut Cursor::new(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")).unwrap(),
            Request::Page
        );
    }

    #[test]
    fn rejects_oversize_duplicate_and_ambiguous_framing() {
        assert!(matches!(
            request(&mut Cursor::new(headers("Content-Length: 4194305\r\n"))),
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
}
