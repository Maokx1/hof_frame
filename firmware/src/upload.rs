use std::io::{self, BufRead, Write};
use std::time::Duration;

use crate::frame::{Corner, Metadata, Orientation};

pub const MAX_UPLOAD_BYTES: usize = 7_000_000;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
pub const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
pub const AWAKE_TIMEOUT: Duration = Duration::from_secs(600);
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const RESULT_GRACE: Duration = Duration::from_secs(6);
// A late-started refresh may use four 120-second BUSY waits after the awake budget.
pub const BROWSER_RESULT_TIMEOUT: Duration =
    Duration::from_secs(AWAKE_TIMEOUT.as_secs() + 600 + RESULT_GRACE.as_secs() + 5);
const MAX_HEADER_BYTES: usize = 8192;

#[derive(Clone, Copy)]
pub enum ProgressStage {
    Received,
    Processed,
    Displaying,
    Displayed,
}

pub enum ProgressEvent<'a> {
    Stage(ProgressStage),
    Success(&'a str),
    Retry(u64, &'a str),
    Fatal(&'a str),
}

pub fn start_progress(stream: &mut impl Write) -> io::Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-hof-progress; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n")?;
    stream.flush()
}

pub fn write_progress(stream: &mut impl Write, event: ProgressEvent<'_>) -> io::Result<()> {
    let (line, terminal) = match event {
        ProgressEvent::Stage(stage) => (
            match stage {
                ProgressStage::Received => "received\n",
                ProgressStage::Processed => "processed\n",
                ProgressStage::Displaying => "displaying\n",
                ProgressStage::Displayed => "displayed\n",
            }
            .to_owned(),
            false,
        ),
        ProgressEvent::Success(message) => {
            (format!("success\t{}\n", progress_message(message)), true)
        }
        ProgressEvent::Retry(seconds, message) => (
            format!("retry\t{seconds}\t{}\n", progress_message(message)),
            true,
        ),
        ProgressEvent::Fatal(message) => (format!("fatal\t{}\n", progress_message(message)), true),
    };
    write!(stream, "{:x}\r\n{line}\r\n", line.len())?;
    if terminal {
        stream.write_all(b"0\r\n\r\n")?;
    }
    stream.flush()
}

fn progress_message(message: &str) -> String {
    message.replace(['\r', '\n', '\t'], " ")
}

pub fn write_response(
    stream: &mut impl Write,
    status: &str,
    kind: &str,
    body: &str,
    remaining: Option<u64>,
) -> io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n", body.len())?;
    if let Some(seconds) = remaining {
        write!(stream, "X-Window-Seconds: {seconds}\r\n")?;
    }
    write!(stream, "\r\n{body}")?;
    stream.flush()
}

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
    Storage(io::Error),
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
    let mut battery_indicator_enabled = None;
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
            ("indicator", value) if battery_indicator_enabled.is_none() => {
                battery_indicator_enabled = Some(match value {
                    "on" => true,
                    "off" => false,
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
            battery_indicator_enabled: battery_indicator_enabled.unwrap_or(false),
        },
    })
}

pub fn receive(
    reader: &mut impl BufRead,
    body: BodyLength,
    destination: &mut [u8],
) -> Result<usize, ReceiveError> {
    let capacity = destination.len().min(MAX_UPLOAD_BYTES);
    receive_to(reader, body, &mut io::Cursor::new(destination), capacity)
}

pub fn receive_to(
    reader: &mut impl BufRead,
    body: BodyLength,
    destination: &mut impl Write,
    capacity: usize,
) -> Result<usize, ReceiveError> {
    let capacity = capacity.min(MAX_UPLOAD_BYTES);
    let mut buffer = vec![0; 4096];
    match body {
        BodyLength::Known(len) => {
            if len > capacity {
                return Err(ReceiveError::TooLarge);
            }
            copy_body(reader, destination, len, &mut buffer)?;
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
                copy_body(reader, destination, size, &mut buffer)?;
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

fn copy_body(
    reader: &mut impl BufRead,
    destination: &mut impl Write,
    mut remaining: usize,
    buffer: &mut [u8],
) -> Result<(), ReceiveError> {
    while remaining != 0 {
        let count = remaining.min(buffer.len());
        reader.read_exact(&mut buffer[..count])?;
        destination
            .write_all(&buffer[..count])
            .map_err(ReceiveError::Storage)?;
        remaining -= count;
    }
    Ok(())
}

pub fn image_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

#[cfg(test)]
#[path = "../tests/unit/upload.rs"]
mod tests;
