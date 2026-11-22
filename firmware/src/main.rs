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

#[cfg(not(target_os = "espidf"))]
fn main() {
    panic!("firmware requires an ESP-IDF target");
}
