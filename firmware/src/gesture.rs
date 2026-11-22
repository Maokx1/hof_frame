use std::time::{Duration, Instant};

pub const BUTTON_DEBOUNCE: Duration = Duration::from_millis(50);
pub const GESTURE_WINDOW: Duration = Duration::from_secs(3);
pub const LONG_HOLD: Duration = Duration::from_secs(3);

/// Requires a stable release before accepting the next waking press.
#[derive(Default)]
pub struct ReleaseGate {
    armed: bool,
    released_since: Option<Instant>,
}

impl ReleaseGate {
    /// Report whether the next press is eligible to start a gesture.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Consume a press only after a full debounce interval of release.
    pub fn sample(&mut self, now: Instant, pressed: bool) -> bool {
        if self.armed && pressed {
            self.armed = false;
            self.released_since = None;
            return true;
        }
        if !self.armed {
            if pressed {
                self.released_since = None;
            } else if let Some(released_at) = self.released_since {
                self.armed = now.duration_since(released_at) >= BUTTON_DEBOUNCE;
            } else {
                self.released_since = Some(now);
            }
        }
        false
    }
}

pub struct FaultSwitch {
    stable_pressed: bool,
    candidate_pressed: bool,
    candidate_since: Instant,
}

impl FaultSwitch {
    /// Initialize debounce state without treating a held boot button as a press.
    pub fn new(now: Instant, initially_pressed: bool) -> Self {
        Self {
            stable_pressed: initially_pressed,
            candidate_pressed: initially_pressed,
            candidate_since: now,
        }
    }

    /// Return true once for each debounced press, requiring release to repeat.
    pub fn sample(&mut self, now: Instant, pressed: bool) -> bool {
        if pressed != self.candidate_pressed {
            self.candidate_pressed = pressed;
            self.candidate_since = now;
        } else if pressed != self.stable_pressed
            && now.duration_since(self.candidate_since) >= BUTTON_DEBOUNCE
        {
            self.stable_pressed = pressed;
            return pressed;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Return to sleep when no complete gesture is recognized.
    Sleep,
    /// Open an upload window after holding the waking press.
    Upload,
    /// Request a manual refresh after three presses.
    ManualRefresh,
}

pub struct GestureClassifier {
    presses: u8,
    stable_pressed: bool,
    candidate_pressed: bool,
    candidate_since: Instant,
    waking_press_held: bool,
    gesture_deadline: Instant,
    hold_deadline: Instant,
    result: Option<Gesture>,
}

impl GestureClassifier {
    /// Count the waking press and start the gesture/hold deadlines.
    pub fn new(wake_at: Instant, initially_pressed: bool) -> Self {
        Self {
            presses: 1,
            stable_pressed: initially_pressed,
            candidate_pressed: initially_pressed,
            candidate_since: wake_at,
            waking_press_held: initially_pressed,
            gesture_deadline: wake_at + GESTURE_WINDOW,
            hold_deadline: wake_at + LONG_HOLD,
            result: None,
        }
    }

    /// Advance debounce and gesture state using caller-supplied time and input.
    pub fn sample(&mut self, now: Instant, raw_pressed: bool) -> Option<Gesture> {
        if self.result.is_some() {
            return self.result;
        }
        if raw_pressed != self.candidate_pressed {
            self.candidate_pressed = raw_pressed;
            self.candidate_since = now;
        } else if self.candidate_pressed != self.stable_pressed
            && now.duration_since(self.candidate_since) >= BUTTON_DEBOUNCE
        {
            self.stable_pressed = self.candidate_pressed;
            if self.stable_pressed {
                self.presses += 1;
                if self.presses == 3 && now <= self.gesture_deadline {
                    self.result = Some(Gesture::ManualRefresh);
                    return self.result;
                }
            } else {
                self.waking_press_held = false;
            }
        }

        // Resolve a pending release before accepting a hold at the deadline.
        if self.waking_press_held
            && self.stable_pressed
            && self.candidate_pressed
            && now >= self.hold_deadline
        {
            self.result = Some(Gesture::Upload);
        }
        if now >= self.gesture_deadline && !(self.waking_press_held && self.stable_pressed) {
            self.result = Some(Gesture::Sleep);
        }
        self.result
    }
}
