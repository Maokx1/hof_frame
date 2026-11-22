//! Firmware entry point: advance the application between states.

#[cfg(target_os = "espidf")]
use firmware::app::Application;

/// Boot the selected mode and drive its state transitions.
#[cfg(target_os = "espidf")]
fn main() {
    let mut application = Application::boot();
    let mut state = application.initial_state();
    loop {
        state = application.advance(state);
    }
}

/// Report that host builds exist only for tests and cannot run the firmware.
#[cfg(not(target_os = "espidf"))]
fn main() {
    eprintln!("firmware requires an ESP-IDF target");
    std::process::exit(1);
}
