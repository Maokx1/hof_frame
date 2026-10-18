use std::io::{self, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, Configuration, EspWifi,
};
use firmware::upload::{self, ProgressEvent, ProgressStage, ReceiveError, Request};

use crate::psram::PsramBuffer;

const PASSWORD: Option<&str> = option_env!("HOF_WIFI_PASSWORD");
const PAGE: &str = include_str!("../assets/upload.html");
pub const AP_ADDRESS: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 168, 4, 1);

pub struct Received {
    pub buffer: PsramBuffer,
    pub len: usize,
    pub metadata: firmware::frame::Metadata,
}

pub enum ProcessingError {
    Retry(String),
    Fatal(String),
}

pub fn setup(
    modem: esp_idf_svc::hal::modem::Modem<'static>,
) -> Result<BlockingWifi<EspWifi<'static>>, esp_idf_svc::sys::EspError> {
    use esp_idf_svc::ipv4;
    use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
    use esp_idf_svc::wifi::WifiDriver;

    let event_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let ap_configuration = NetifConfiguration {
        ip_configuration: Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
            subnet: ipv4::Subnet {
                gateway: AP_ADDRESS,
                mask: "24".parse().expect("valid fixed subnet mask"),
            },
            dhcp_enabled: true,
            dns: None,
            secondary_dns: None,
        })),
        ..NetifConfiguration::wifi_default_router()
    };
    let wifi = EspWifi::wrap_all(
        WifiDriver::new(modem, event_loop.clone(), None)?,
        EspNetif::new(NetifStack::Sta)?,
        EspNetif::new_with_conf(&ap_configuration)?,
    )?;
    let mut wifi = BlockingWifi::wrap(wifi, event_loop)?;
    configure(&mut wifi)?;
    Ok(wifi)
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
    response_with_window(stream, status, kind, body, None)
}

fn response_with_window(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &str,
    remaining: Option<u64>,
) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    upload::write_response(stream, status, kind, body, remaining)
}

fn reject(stream: &mut TcpStream, error: &ReceiveError, remaining: u64) {
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
    if let Err(error) = response_with_window(stream, status, "text/plain", message, Some(remaining))
    {
        log::warn!("upload rejection response could not be sent: {error}");
    }
}

pub fn configure(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
) -> Result<(), esp_idf_svc::sys::EspError> {
    let password = PASSWORD.filter(|value| upload::valid_wifi_password(value))
        .ok_or_else(|| {
            log::error!("upload requires HOF_WIFI_PASSWORD at build time: 8-63 ASCII characters; never log or commit it");
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
    safety: &mut firmware::safety::SafetyGuard<crate::safety_nvs::NvsSafetyStore>,
    schedule: &mut firmware::schedule::RefreshSchedule,
) -> Result<(), Box<dyn std::error::Error>> {
    let awake_deadline = boot + upload::AWAKE_TIMEOUT;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        crate::set_work_phase(safety, firmware::safety::WorkPhase::Upload)?;
        if Instant::now() >= awake_deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "awake deadline expired before AP startup",
            )
            .into());
        }
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
        log::info!("UploadWindow: AP HOF-Frame ready; open http://{address}; WPA2; one client only; idle=300 s; panel OFF");
        let idle_deadline = (Instant::now() + upload::IDLE_TIMEOUT).min(awake_deadline);
        loop {
            if Instant::now() >= idle_deadline || Instant::now() >= awake_deadline {
                log::info!("UploadWindow: idle deadline; no upload; saved frame and daily deadline unchanged");
                return Ok(());
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
                    reject(
                        &mut reader.get_mut().stream,
                        &error,
                        idle_deadline
                            .saturating_duration_since(Instant::now())
                            .as_secs(),
                    );
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
                            .replace("__PROCESS_TIMEOUT_MS__", &upload::BROWSER_RESULT_TIMEOUT.as_millis().to_string())
                            .replace("__DISPLAY_MODE__", "A successful save triggers a REAL screen refresh. Wi-Fi stays on for the final result, then closes six seconds after success. Keep stable power connected."),
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
                            0,
                        );
                        return Ok(());
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
                                "Not enough upload memory; saved frame unchanged. Further work is blocked; hotspot closing.",
                            ) {
                                log::warn!("UploadWindow: allocation failure response failed: {response_error}");
                            }
                            FreeRtos::delay_ms(upload::RESULT_GRACE.as_millis() as u32);
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
                            reject(
                                &mut reader.get_mut().stream,
                                &error,
                                idle_deadline
                                    .saturating_duration_since(Instant::now())
                                    .as_secs(),
                            );
                            continue;
                        }
                    };
                    if !upload::image_signature(&buffer.bytes_mut()[..len]) {
                        log::warn!("UploadWindow: PNG/JPEG signature missing");
                        reject(
                            &mut reader.get_mut().stream,
                            &ReceiveError::UnsupportedImage,
                            idle_deadline
                                .saturating_duration_since(Instant::now())
                                .as_secs(),
                        );
                        continue;
                    }
                    log::info!("UploadWindow: RECEIVED {len} bytes; processing with AP ON; streaming progress and final result");
                    let stream = &mut reader.get_mut().stream;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    let mut connected = match upload::start_progress(stream) {
                        Ok(()) => true,
                        Err(error) => {
                            log::warn!("UploadWindow: progress response could not be started: {error}; continuing image work");
                            false
                        }
                    };
                    let mut notify = |stage| {
                        if connected {
                            if let Err(error) =
                                upload::write_progress(stream, ProgressEvent::Stage(stage))
                            {
                                log::warn!("UploadWindow: progress connection lost: {error}; continuing image work");
                                connected = false;
                            }
                        }
                    };
                    notify(ProgressStage::Received);
                    let outcome = crate::process_upload(
                        Received {
                            buffer,
                            len,
                            metadata,
                        },
                        awake_deadline,
                        &mut notify,
                        safety,
                        schedule,
                    );
                    #[cfg(not(feature = "usb"))]
                    if outcome.is_ok() {
                        crate::update_retained_state(|state| {
                            state.schedule = *schedule;
                            state.refresh_in_progress = 0;
                        });
                    }
                    let remaining = idle_deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs();
                    match outcome {
                        Ok(message) => {
                            if connected {
                                if let Err(error) =
                                    upload::write_progress(stream, ProgressEvent::Success(message))
                                {
                                    log::warn!("UploadWindow: work succeeded but final response could not be delivered: {error}");
                                }
                            }
                            log::info!("UploadWindow: final success; closing AP after six-second result grace");
                            FreeRtos::delay_ms(upload::RESULT_GRACE.as_millis() as u32);
                            return Ok(());
                        }
                        Err(ProcessingError::Retry(message)) => {
                            log::warn!("UploadWindow: processing rejected: {message}; retry window remaining={remaining} s");
                            if connected {
                                if let Err(error) = upload::write_progress(
                                    stream,
                                    ProgressEvent::Retry(remaining, &message),
                                ) {
                                    log::warn!("UploadWindow: failure response could not be delivered: {error}");
                                }
                            }
                            crate::set_work_phase(safety, firmware::safety::WorkPhase::Upload)?;
                        }
                        Err(ProcessingError::Fatal(message)) => {
                            log::error!("UploadWindow: processing safety fault: {message}");
                            if connected {
                                if let Err(error) = upload::write_progress(
                                    stream,
                                    ProgressEvent::Fatal(&format!(
                                        "{message} Further work is blocked; hotspot closing."
                                    )),
                                ) {
                                    log::warn!("UploadWindow: fatal response could not be delivered: {error}");
                                }
                            }
                            FreeRtos::delay_ms(upload::RESULT_GRACE.as_millis() as u32);
                            return Err(message.into());
                        }
                    }
                }
            }
        }
    })();
    let cleanup_phase = if result.is_ok() {
        crate::set_work_phase(safety, firmware::safety::WorkPhase::RadioCleanup)
    } else {
        Ok(())
    };
    let stopped = wifi.stop();
    if let Err(error) = cleanup_phase {
        if let Err(stop_error) = stopped {
            log::error!(
                "UploadWindow: diagnostic phase write and Wi-Fi shutdown both failed: {stop_error}"
            );
        }
        return Err(error.into());
    }
    match (result, stopped) {
        (Err(error), Err(stop_error)) => {
            log::error!("UploadWindow: Wi-Fi shutdown also failed: {stop_error:?}");
            Err(error)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(received), Ok(())) => {
            crate::set_work_phase(safety, firmware::safety::WorkPhase::Idle)?;
            log::info!("UploadWindow: sockets closed; Wi-Fi OFF");
            Ok(received)
        }
    }
}
