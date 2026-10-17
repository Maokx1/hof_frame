//! GPIO probe: find out which GPIO (if any) a button is wired to.
//!
//! Watches every GPIO that is free on the ESP32-133C02 board and logs each
//! level change. Alternates between internal pull-up (catches a switch to GND)
//! and internal pull-down (catches a switch to 3V3). Press SW7 / SW8 while it
//! runs and watch the log. If nothing is logged, the switch does not reach any
//! probed GPIO (most likely it only drives another chip, e.g. the IP5306).
//!
//! Run with: cargo run --release --bin gpio_probe

use std::thread::sleep;
use std::time::{Duration, Instant};

use esp_idf_svc::sys::{self, esp};

/// Free GPIOs only. Excluded: display (6, 7, 9, 17, 18, 40, 41, 45), native
/// USB (19, 20), and the flash/PSRAM pins of the N16R8 module (26-37).
const PROBED: [i32; 21] = [
    0, 1, 2, 3, 4, 5, 8, 10, 11, 12, 13, 14, 15, 16, 21, 38, 39, 42, 43, 44, 46,
];
const PHASE: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(2);
const DEBOUNCE_POLLS: u8 = 10;

fn configure(pull_up: bool) {
    let mode = if pull_up {
        sys::gpio_pull_mode_t_GPIO_PULLUP_ONLY
    } else {
        sys::gpio_pull_mode_t_GPIO_PULLDOWN_ONLY
    };
    for &pin in &PROBED {
        unsafe {
            esp!(sys::gpio_reset_pin(pin)).unwrap();
            esp!(sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_INPUT)).unwrap();
            esp!(sys::gpio_set_pull_mode(pin, mode)).unwrap();
        }
    }
    // Let the pulls settle before taking the baseline.
    sleep(Duration::from_millis(50));
}

fn level(pin: i32) -> u8 {
    unsafe { sys::gpio_get_level(pin) as u8 }
}

fn run_phase(pull_up: bool) {
    let name = if pull_up { "PULL-UP (switch to GND)" } else { "PULL-DOWN (switch to 3V3)" };
    configure(pull_up);

    let mut stable: Vec<u8> = PROBED.iter().map(|&p| level(p)).collect();
    let mut pending = vec![0u8; PROBED.len()];

    let idle: Vec<String> = PROBED
        .iter()
        .zip(&stable)
        .map(|(p, l)| format!("{p}={l}"))
        .collect();
    log::info!("=== {name}: press SW7 / SW8 now ({} s) ===", PHASE.as_secs());
    log::info!("idle levels: {}", idle.join(" "));

    let start = Instant::now();
    while start.elapsed() < PHASE {
        for (i, &pin) in PROBED.iter().enumerate() {
            let now = level(pin);
            if now == stable[i] {
                pending[i] = 0;
                continue;
            }
            pending[i] += 1;
            if pending[i] >= DEBOUNCE_POLLS {
                log::warn!(
                    ">>> GPIO{pin} changed {} -> {} [{name}] at {} ms",
                    stable[i],
                    now,
                    start.elapsed().as_millis()
                );
                stable[i] = now;
                pending[i] = 0;
            }
        }
        sleep(POLL);
    }
}

fn main() {
    // Required once so esp-idf-sys runtime patches link properly.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("gpio_probe starting; probing {} GPIOs", PROBED.len());
    loop {
        run_phase(true);
        run_phase(false);
    }
}
