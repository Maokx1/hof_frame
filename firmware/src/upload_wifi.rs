use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, Configuration, EspWifi,
};
use firmware::upload::{self, ReceiveError, Request};

use crate::psram::PsramBuffer;

const PASSWORD: Option<&str> = option_env!("HOF_WIFI_PASSWORD");
const PAGE: &str = include_str!("../assets/upload.html");
pub const AP_ADDRESS: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 168, 4, 1);

pub struct Received {
    pub buffer: PsramBuffer,
    pub len: usize,
    pub metadata: firmware::frame::Metadata,
}

struct TimedStream {
    stream: TcpStream,
    deadline: Instant,
    last_data: Instant,
    inactivity: Duration,
}

impl Read for TimedStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            let now = Instant::now();
            if upload::receive_timed_out(now, self.deadline, self.last_data, self.inactivity) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "receive deadline exceeded",
                ));
            }
            match self.stream.read(bytes) {
                Ok(len) => {
                    if len != 0 {
                        self.last_data = Instant::now();
                    }
                    FreeRtos::delay_ms(1);
                    return Ok(len);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

fn response(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n{body}", body.len())?;
    stream.flush()
}

fn reject(stream: &mut TcpStream, error: &ReceiveError) {
    let (status, message) = match error {
        ReceiveError::TooLarge => (
            "413 Payload Too Large",
            "Upload exceeds 4 MiB; saved frame unchanged.",
        ),
        ReceiveError::UnsupportedImage => (
            "415 Unsupported Media Type",
            "Only PNG/JPEG accepted; saved frame unchanged.",
        ),
        ReceiveError::Io(error) if error.kind() == io::ErrorKind::TimedOut => (
            "408 Request Timeout",
            "Upload deadline exceeded; saved frame unchanged.",
        ),
        _ => (
            "400 Bad Request",
            "Invalid or interrupted request; saved frame unchanged.",
        ),
    };
    if let Err(error) = response(stream, status, "text/plain", message) {
        log::warn!("upload rejection response could not be sent: {error}");
    }
}

pub fn configure(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
) -> Result<(), esp_idf_svc::sys::EspError> {
    let password = PASSWORD.filter(|value| upload::valid_wifi_password(value))
        .ok_or_else(|| {
            log::error!("debug-upload requires HOF_WIFI_PASSWORD at build time: 8-63 ASCII characters; never log or commit it");
            esp_idf_svc::sys::EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_ARG).unwrap()
        })?;
    wifi.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
        ssid: "HOF-Frame".try_into().expect("SSID fits"),
        password: password.try_into().expect("validated WPA2 password fits"),
        auth_method: AuthMethod::WPA2Personal,
        max_connections: 1,
        channel: 6,
        ..Default::default()
    }))
}

pub fn run(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    boot: Instant,
) -> Result<Option<Received>, Box<dyn std::error::Error>> {
    let awake_deadline = boot + upload::AWAKE_TIMEOUT;
    let result = (|| -> Result<Option<Received>, Box<dyn std::error::Error>> {
        wifi.start()?;
        wifi.wait_netif_up()?;
        let address = wifi.wifi().ap_netif().get_ip_info()?.ip;
        if address != AP_ADDRESS {
            return Err(io::Error::other(format!(
                "AP address mismatch: expected {AP_ADDRESS}, got {address}"
            ))
            .into());
        }
        let listener = TcpListener::bind(("0.0.0.0", 80))?;
        listener.set_nonblocking(true)?;
        log::info!("UploadWindow: AP HOF-Frame ready; open http://{address}; WPA2; one client only; idle=300 s; panel OFF; sleep mocked");
        let idle_deadline = Instant::now() + upload::IDLE_TIMEOUT;
        loop {
            if Instant::now() >= idle_deadline || Instant::now() >= awake_deadline {
                log::info!("UploadWindow: idle deadline; no upload; saved frame and daily deadline unchanged");
                return Ok(None);
            }
            let (stream, _) = match listener.accept() {
                Ok(client) => client,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    FreeRtos::delay_ms(20);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            stream.set_read_timeout(Some(Duration::from_millis(250)))?;
            let mut reader = BufReader::with_capacity(
                2048,
                TimedStream {
                    stream,
                    deadline: (Instant::now() + upload::HEADER_TIMEOUT)
                        .min(idle_deadline)
                        .min(awake_deadline),
                    last_data: Instant::now(),
                    inactivity: upload::HEADER_TIMEOUT,
                },
            );
            let request = match upload::request(&mut reader) {
                Ok(request) => request,
                Err(error) => {
                    log::warn!("UploadWindow: headers rejected: {error:?}; closing this connection only; hotspot idle deadline unchanged");
                    reject(&mut reader.get_mut().stream, &error);
                    continue;
                }
            };
            match request {
                Request::NotFound => {
                    if let Err(error) = response(
                        &mut reader.get_mut().stream,
                        "404 Not Found",
                        "text/plain",
                        "Not found",
                    ) {
                        log::warn!("UploadWindow: 404 response failed: {error}");
                    }
                }
                Request::Page => {
                    let remaining = idle_deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs();
                    if let Err(error) = response(
                        &mut reader.get_mut().stream,
                        "200 OK",
                        "text/html; charset=utf-8",
                        &PAGE
                            .replace("__SECONDS__", &remaining.to_string())
                            .replace("__DISPLAY_MODE__", if cfg!(feature = "debug-upload-display") {
                                "A successful save triggers a REAL screen refresh. Keep bench power connected until completion; deep sleep stays mocked."
                            } else {
                                "Screen refresh stays mocked."
                            }),
                    ) {
                        log::warn!("UploadWindow: page response failed: {error}");
                    }
                }
                Request::Upload { body, metadata } => {
                    if Instant::now() >= idle_deadline || Instant::now() >= awake_deadline {
                        reject(
                            &mut reader.get_mut().stream,
                            &ReceiveError::Io(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "idle deadline",
                            )),
                        );
                        return Ok(None);
                    }
                    log::info!("UploadWindow: transfer started; {body:?}, {metadata:?}; max=4194304 bytes, inactivity=120 s, absolute=120 s");
                    // Request framing/declared size was checked before allocating.
                    let capacity = match body {
                        upload::BodyLength::Known(len) => len.max(1),
                        upload::BodyLength::Chunked => upload::MAX_UPLOAD_BYTES,
                    };
                    let transfer_started = Instant::now();
                    let mut buffer = match PsramBuffer::new(capacity) {
                        Ok(buffer) => buffer,
                        Err(error) => {
                            if let Err(response_error) = response(
                                &mut reader.get_mut().stream,
                                "503 Service Unavailable",
                                "text/plain",
                                "Not enough upload memory; saved frame unchanged.",
                            ) {
                                log::warn!("UploadWindow: allocation failure response failed: {response_error}");
                            }
                            return Err(error.into());
                        }
                    };
                    let timed = reader.get_mut();
                    timed.deadline =
                        (transfer_started + upload::TRANSFER_TIMEOUT).min(awake_deadline);
                    timed.inactivity = upload::INACTIVITY_TIMEOUT;
                    timed.last_data = Instant::now();
                    let len = match upload::receive(&mut reader, body, buffer.bytes_mut()) {
                        Ok(len) => len,
                        Err(error) => {
                            log::warn!("UploadWindow: receive failed: {error:?}");
                            reject(&mut reader.get_mut().stream, &error);
                            return Ok(None);
                        }
                    };
                    if !upload::image_signature(&buffer.bytes_mut()[..len]) {
                        log::warn!("UploadWindow: PNG/JPEG signature missing");
                        reject(
                            &mut reader.get_mut().stream,
                            &ReceiveError::UnsupportedImage,
                        );
                        return Ok(None);
                    }
                    let receipt = if cfg!(feature = "debug-upload-display") {
                        "Upload received. Wi-Fi is closing before decoding, saving, and REAL screen refresh. Receipt is not confirmation of save/display success; keep power connected and check serial logs for display test: SUCCESS."
                    } else {
                        "Upload received. Wi-Fi is closing before decoding and saving. This confirms receipt only, not successful save; check serial logs for SAVED or failure. Screen refresh remains mocked."
                    };
                    response(
                        &mut reader.get_mut().stream,
                        "200 OK",
                        "text/plain",
                        receipt,
                    )?;
                    log::info!("UploadWindow: RECEIVED {len} bytes in PSRAM; receipt response sent; decode/commit follows radio shutdown; no panel refresh");
                    // Allow the response to reach the client before radio teardown.
                    FreeRtos::delay_ms(500);
                    return Ok(Some(Received {
                        buffer,
                        len,
                        metadata,
                    }));
                }
            }
        }
    })();
    let stopped = wifi.stop();
    match (result, stopped) {
        (Err(error), Err(stop_error)) => {
            log::error!("UploadWindow: Wi-Fi shutdown also failed: {stop_error:?}");
            Err(error)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(received), Ok(())) => {
            log::info!("UploadWindow: sockets closed; Wi-Fi OFF; USB remains connected");
            Ok(received)
        }
    }
}
