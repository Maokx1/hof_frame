//! Bounded upload HTTP session; transport and request handlers stay separate.

use std::io::{self, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::{
    frame::Metadata,
    safety::WorkPhase,
    schedule::RefreshSchedule,
    partition::FlashError,
    safety_nvs::NvsSafetyError,
    upload::{self, BodyLength, ProgressEvent, ProgressStage, ReceiveError, Request},
    upload_flash::FlashUpload,
    work::{self, Guard},
};
use esp_idf_svc::{
    hal::delay::FreeRtos,
    mdns::EspMdns,
    sys::EspError,
    wifi::{AccessPointConfiguration, AuthMethod, BlockingWifi, Configuration, EspWifi},
};

const PASSWORD: Option<&str> = option_env!("HOF_WIFI_PASSWORD");
const PAGE: &str = include_str!("../assets/upload.html");
pub const AP_ADDRESS: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 168, 4, 1);
pub const AP_HOSTNAME: &str = "bursztynki";
type Wifi = BlockingWifi<EspWifi<'static>>;
type Reader = BufReader<TimedStream>;
type SessionResult<T> = Result<T, UploadError>;

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The build has no valid hotspot password.
    #[error("upload requires HOF_WIFI_PASSWORD at build time: 8-63 ASCII characters")]
    MissingPassword,
    /// The SSID or password does not fit the Wi-Fi configuration.
    #[error("hotspot SSID or password is too long")]
    CredentialsTooLong,
    /// The awake budget ran out before the hotspot started.
    #[error("awake deadline expired before AP startup")]
    DeadlineExpired,
    /// The hotspot came up on an unexpected address.
    #[error("AP address mismatch: expected {AP_ADDRESS}, got {0}")]
    AddressMismatch(std::net::Ipv4Addr),
    /// The upload staging partition is unavailable.
    #[error("upload storage unavailable: {0}")]
    StagingUnavailable(#[from] FlashError),
    /// Writing the received body to staging flash failed.
    #[error("upload staging failed: {0}")]
    StagingWrite(io::Error),
    /// Image processing failed in a way that blocks further work.
    #[error("{0}")]
    Processing(String),
    /// A safety diagnostic boundary could not be committed.
    #[error("{0}")]
    Safety(#[from] NvsSafetyError),
    /// A Wi-Fi, network-interface, or mDNS call failed.
    #[error("Wi-Fi call failed: {0}")]
    Esp(#[from] EspError),
    /// A socket operation failed.
    #[error("socket failed: {0}")]
    Io(#[from] io::Error),
}

pub struct Received {
    pub buffer: FlashUpload,
    pub len: usize,
    pub metadata: Metadata,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessingError {
    /// The image was rejected and the upload window can accept another attempt.
    #[error("{0}")]
    Retry(String),
    /// A safety or processing failure prevents further work.
    #[error("{0}")]
    Fatal(String),
    /// The display refresh failed after cleanup, but USB operation can continue.
    #[cfg(feature = "usb")]
    #[error("{0}")]
    DisplayFailed(String),
}

enum Flow {
    /// Continue serving requests in the upload window.
    Continue,
    /// Close the upload window.
    Close,
}

struct Session<'a> {
    safety: &'a mut Guard,
    schedule: &'a mut RefreshSchedule,
    awake_deadline: Instant,
    idle_deadline: Instant,
    battery_percentage: Option<u8>,
}

/// Configure the fixed AP network without starting its radio.
pub fn setup(modem: esp_idf_svc::hal::modem::Modem<'static>) -> Result<Wifi, UploadError> {
    use esp_idf_svc::{
        ipv4,
        netif::{EspNetif, NetifConfiguration, NetifStack},
        wifi::WifiDriver,
    };
    let event_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let ap_configuration = NetifConfiguration {
        ip_configuration: Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
            subnet: ipv4::Subnet {
                gateway: AP_ADDRESS,
                mask: ipv4::Mask(24),
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

/// Validate build-time credentials and configure a one-client WPA2 hotspot.
pub fn configure(wifi: &mut Wifi) -> Result<(), UploadError> {
    let password = PASSWORD
        .filter(|value| upload::valid_wifi_password(value))
        .ok_or(UploadError::MissingPassword)?;
    wifi.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
        ssid: "HOF-Frame"
            .try_into()
            .map_err(|_| UploadError::CredentialsTooLong)?,
        password: password
            .try_into()
            .map_err(|_| UploadError::CredentialsTooLong)?,
        auth_method: AuthMethod::WPA2Personal,
        max_connections: 1,
        channel: 6,
        ..Default::default()
    }))?;
    Ok(())
}

/// Serve a bounded window, always attempting radio cleanup after session exit.
pub fn run(
    wifi: &mut Wifi,
    boot: Instant,
    safety: &mut Guard,
    schedule: &mut RefreshSchedule,
    battery_percentage: Option<u8>,
) -> SessionResult<()> {
    let awake_deadline = boot + upload::AWAKE_TIMEOUT;
    let result = serve(wifi, safety, schedule, awake_deadline, battery_percentage);
    stop(wifi, safety, result)
}

/// Start mDNS/radio, verify the AP address, and own sockets until serving ends.
fn serve(
    wifi: &mut Wifi,
    safety: &mut Guard,
    schedule: &mut RefreshSchedule,
    awake_deadline: Instant,
    battery_percentage: Option<u8>,
) -> SessionResult<()> {
    work::set_phase(safety, WorkPhase::Upload)?;
    if Instant::now() >= awake_deadline {
        return Err(UploadError::DeadlineExpired);
    }
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(AP_HOSTNAME)?;
    wifi.start()?;
    wifi.wait_netif_up()?;
    let address = wifi.wifi().ap_netif().get_ip_info()?.ip;
    if address != AP_ADDRESS {
        return Err(UploadError::AddressMismatch(address));
    }
    let listener = TcpListener::bind(("0.0.0.0", 80))?;
    listener.set_nonblocking(true)?;
    log::info!("UploadWindow: AP HOF-Frame ready; open http://{AP_HOSTNAME}.local; WPA2; one client only; idle=300 s; panel OFF");
    Session {
        safety,
        schedule,
        awake_deadline,
        idle_deadline: (Instant::now() + upload::IDLE_TIMEOUT).min(awake_deadline),
        battery_percentage,
    }
    .listen(&listener)
}

/// Stop the AP even on failures, preserving the primary diagnostic/error.
fn stop(wifi: &mut Wifi, safety: &mut Guard, result: SessionResult<()>) -> SessionResult<()> {
    let phase = if result.is_ok() {
        work::set_phase(safety, WorkPhase::RadioCleanup)
    } else {
        Ok(())
    };
    let stopped = wifi.stop();
    if let Err(error) = phase {
        if let Err(stop_error) = stopped {
            log::error!(
                "UploadWindow: diagnostic phase write and Wi-Fi shutdown both failed: {stop_error}"
            );
        }
        return Err(error.into());
    }
    match (result, stopped) {
        (Err(error), Err(stop_error)) => {
            log::error!("UploadWindow: Wi-Fi shutdown also failed: {stop_error}");
            Err(error)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(())) => {
            work::set_phase(safety, WorkPhase::Idle)?;
            log::info!("UploadWindow: sockets closed; Wi-Fi OFF");
            Ok(())
        }
    }
}

impl Session<'_> {
    /// Accept connections without allowing page loads or retries to extend deadlines.
    fn listen(&mut self, listener: &TcpListener) -> SessionResult<()> {
        loop {
            if self.expired() {
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
            let mut reader = request_reader(stream, self.idle_deadline, self.awake_deadline)?;
            let request = match upload::request(&mut reader) {
                Ok(request) => request,
                Err(error) => {
                    log::warn!("UploadWindow: headers rejected: {error:?}; closing this connection only; hotspot idle deadline unchanged");
                    reject(&mut reader.get_mut().stream, &error, self.remaining());
                    continue;
                }
            };
            if let Flow::Close = self.handle(request, &mut reader)? {
                return Ok(());
            }
        }
    }

    /// Dispatch each route to one handler; only an upload can end the window.
    fn handle(&mut self, request: Request, reader: &mut Reader) -> SessionResult<Flow> {
        match request {
            Request::NotFound => {
                respond(
                    &mut reader.get_mut().stream,
                    "404 Not Found",
                    "text/plain",
                    "Not found",
                    "404 response",
                );
                Ok(Flow::Continue)
            }
            Request::Page => {
                respond(
                    &mut reader.get_mut().stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    &render_page(self.remaining(), self.battery_percentage),
                    "page response",
                );
                Ok(Flow::Continue)
            }
            Request::Upload { body, metadata } => self.upload(reader, body, metadata),
        }
    }

    /// Reject expired starts, stage valid bodies, and then process with AP still on.
    fn upload(
        &mut self,
        reader: &mut Reader,
        body: BodyLength,
        metadata: Metadata,
    ) -> SessionResult<Flow> {
        if self.expired() {
            reject(
                &mut reader.get_mut().stream,
                &ReceiveError::Io(io::Error::new(io::ErrorKind::TimedOut, "idle deadline")),
                0,
            );
            return Ok(Flow::Close);
        }
        let Some(received) = self.receive(reader, body, metadata)? else {
            return Ok(Flow::Continue);
        };
        log::info!("UploadWindow: RECEIVED {} bytes; processing with AP ON; streaming progress and final result",
            received.len);
        let mut progress = ProgressConnection::start(&mut reader.get_mut().stream)?;
        progress.send(ProgressEvent::Stage(ProgressStage::Received));
        let outcome = crate::pipeline::process(
            received,
            self.awake_deadline,
            &mut |stage| progress.send(ProgressEvent::Stage(stage)),
            self.safety,
            self.schedule,
        );
        record_success(&outcome, *self.schedule);
        self.complete(outcome, &mut progress)
    }

    /// Stream into flash under transfer bounds; only storage faults close the AP.
    fn receive(
        &self,
        reader: &mut Reader,
        body: BodyLength,
        metadata: Metadata,
    ) -> SessionResult<Option<Received>> {
        log::info!("UploadWindow: transfer started; {body:?}, {metadata:?}; max=7000000 bytes, flash staging, inactivity=120 s, absolute=120 s");
        let deadline = (Instant::now() + upload::TRANSFER_TIMEOUT).min(self.awake_deadline);
        let mut buffer = match FlashUpload::open(deadline) {
            Ok(buffer) => buffer,
            Err(error) => {
                storage_unavailable(&mut reader.get_mut().stream, "Upload storage unavailable; saved frame unchanged. Further work is blocked; hotspot closing.");
                return Err(error.into());
            }
        };
        let timed = reader.get_mut();
        timed.deadline = deadline;
        timed.inactivity = upload::INACTIVITY_TIMEOUT;
        timed.last_data = Instant::now();
        let len = match upload::receive_to(reader, body, &mut buffer, upload::MAX_UPLOAD_BYTES) {
            Ok(len) => len,
            Err(ReceiveError::Storage(error)) if error.kind() != io::ErrorKind::TimedOut => {
                log::error!("UploadWindow: flash staging failed: {error}");
                storage_unavailable(&mut reader.get_mut().stream, "Upload storage failed; saved frame unchanged. Further work is blocked; hotspot closing.");
                return Err(UploadError::StagingWrite(error));
            }
            Err(error) => {
                log::warn!("UploadWindow: receive failed: {error:?}");
                reject(&mut reader.get_mut().stream, &error, self.remaining());
                return Ok(None);
            }
        };
        let mut signature = [0; 8];
        buffer
            .reader(self.awake_deadline)
            .read_exact(&mut signature[..len.min(8)])?;
        if !upload::image_signature(&signature[..len.min(8)]) {
            log::warn!("UploadWindow: PNG/JPEG signature missing");
            reject(
                &mut reader.get_mut().stream,
                &ReceiveError::UnsupportedImage,
                self.remaining(),
            );
            return Ok(None);
        }
        Ok(Some(Received {
            buffer,
            len,
            metadata,
        }))
    }

    /// Send the terminal result and preserve bounded retries for rejected images.
    fn complete(
        &mut self,
        outcome: Result<&str, ProcessingError>,
        progress: &mut ProgressConnection<'_>,
    ) -> SessionResult<Flow> {
        match outcome {
            Ok(message) => {
                progress.send(ProgressEvent::Success(message));
                log::info!("UploadWindow: final success; closing AP after six-second result grace");
                result_grace();
                Ok(Flow::Close)
            }
            Err(ProcessingError::Retry(message)) => {
                let remaining = self.remaining();
                log::warn!("UploadWindow: processing rejected: {message}; retry window remaining={remaining} s");
                progress.send(ProgressEvent::Retry(remaining, &message));
                work::set_phase(self.safety, WorkPhase::Upload)?;
                Ok(Flow::Continue)
            }
            #[cfg(feature = "usb")]
            Err(ProcessingError::DisplayFailed(message)) => {
                log::error!("UploadWindow: {message}");
                progress.send(ProgressEvent::Fatal(&message));
                result_grace();
                Ok(Flow::Close)
            }
            Err(ProcessingError::Fatal(message)) => {
                log::error!("UploadWindow: processing safety fault: {message}");
                progress.send(ProgressEvent::Fatal(&format!(
                    "{message} Further work is blocked; hotspot closing."
                )));
                result_grace();
                Err(UploadError::Processing(message))
            }
        }
    }

    /// Read the remaining original retry window without extending it.
    fn remaining(&self) -> u64 {
        self.idle_deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
    }

    /// Check both session deadlines before accepting a new upload.
    fn expired(&self) -> bool {
        let now = Instant::now();
        now >= self.idle_deadline || now >= self.awake_deadline
    }
}

struct ProgressConnection<'a> {
    stream: &'a mut TcpStream,
    connected: bool,
}

impl<'a> ProgressConnection<'a> {
    /// Start progress without cancelling already accepted image work on disconnect.
    fn start(stream: &'a mut TcpStream) -> io::Result<Self> {
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let connected = match upload::start_progress(stream) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("UploadWindow: progress response could not be started: {error}; continuing image work");
                false
            }
        };
        Ok(Self { stream, connected })
    }

    /// Log a lost connection once; transport failure does not cancel image work.
    fn send(&mut self, event: ProgressEvent<'_>) {
        if self.connected {
            if let Err(error) = upload::write_progress(self.stream, event) {
                log::warn!(
                    "UploadWindow: progress/result connection lost: {error}; continuing image work"
                );
                self.connected = false;
            }
        }
    }
}

struct TimedStream {
    stream: TcpStream,
    deadline: Instant,
    last_data: Instant,
    inactivity: Duration,
}

impl Read for TimedStream {
    /// Retry socket polling while enforcing absolute and inactivity deadlines.
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            if upload::receive_timed_out(
                Instant::now(),
                self.deadline,
                self.last_data,
                self.inactivity,
            ) {
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

/// Configure a buffered header reader capped by both session deadlines.
fn request_reader(stream: TcpStream, idle: Instant, awake: Instant) -> io::Result<Reader> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    Ok(BufReader::with_capacity(
        2048,
        TimedStream {
            stream,
            deadline: (Instant::now() + upload::HEADER_TIMEOUT)
                .min(idle)
                .min(awake),
            last_data: Instant::now(),
            inactivity: upload::HEADER_TIMEOUT,
        },
    ))
}

/// Compose the self-contained page from immutable assets and current display data.
fn render_page(remaining: u64, percentage: Option<u8>) -> String {
    PAGE.replace("/* __STYLES__ */", include_str!("../assets/upload.css"))
        .replace(
            "/* __TRANSLATIONS__ */",
            include_str!("../assets/upload-i18n.js"),
        )
        .replace("/* __SCRIPT__ */", include_str!("../assets/upload.js"))
        .replace("__SECONDS__", &remaining.to_string())
        .replace(
            "__BATTERY_PERCENTAGE__",
            &percentage.map_or_else(|| "?".to_owned(), |value| format!("~{value}")),
        )
        .replace(
            "__PROCESS_TIMEOUT_MS__",
            &upload::BROWSER_RESULT_TIMEOUT.as_millis().to_string(),
        )
}

/// Write a response under a bounded socket write timeout.
fn response(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &str,
    remaining: Option<u64>,
) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    upload::write_response(stream, status, kind, body, remaining)
}

/// Send noncritical page/error responses, explicitly logging transport failures.
fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &str, context: &str) {
    if let Err(error) = response(stream, status, kind, body, None) {
        log::warn!("UploadWindow: {context} failed: {error}");
    }
}

/// Give the browser a bounded grace period before shutting down the radio.
fn result_grace() {
    FreeRtos::delay_ms(upload::RESULT_GRACE.as_millis() as u32);
}

production_items! {
    /// Retain a successful production upload before the browser hears about it.
    fn record_success(outcome: &Result<&str, ProcessingError>, schedule: RefreshSchedule) {
        if outcome.is_ok() {
            crate::rtc::upload_succeeded(schedule);
        }
    }
}

usb_items! {
    /// USB mode keeps its schedule in RAM, so there is no RTC upload record.
    fn record_success(_: &Result<&str, ProcessingError>, _: RefreshSchedule) {}
}

/// Report staging faults before closing the AP and blocking further work.
fn storage_unavailable(stream: &mut TcpStream, message: &str) {
    respond(
        stream,
        "503 Service Unavailable",
        "text/plain",
        message,
        "storage failure response",
    );
    result_grace();
}

/// Report retryable receive rejection and the unchanged remaining upload window.
fn reject(stream: &mut TcpStream, error: &ReceiveError, remaining: u64) {
    let (status, message) = match error {
        ReceiveError::TooLarge => (
            "413 Payload Too Large",
            "Upload exceeds 7 MB (7,000,000 bytes); saved frame unchanged.",
        ),
        ReceiveError::UnsupportedImage => (
            "415 Unsupported Media Type",
            "Only PNG/JPEG accepted; saved frame unchanged.",
        ),
        ReceiveError::Io(error) | ReceiveError::Storage(error)
            if error.kind() == io::ErrorKind::TimedOut =>
        {
            (
                "408 Request Timeout",
                "Upload deadline exceeded; saved frame unchanged.",
            )
        }
        _ => (
            "400 Bad Request",
            "Invalid or interrupted request; saved frame unchanged.",
        ),
    };
    if let Err(error) = response(stream, status, "text/plain", message, Some(remaining)) {
        log::warn!("upload rejection response could not be sent: {error}");
    }
}
