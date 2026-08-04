use ec::input::{is_candidate, VIRTUAL_NAME};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Event {
    Move(i32, i32),
    Wheel(i32, i32),
    Button(u16, bool),
    Disconnect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Output {
    Forward(Event),
    Hold(bool),
    Error,
}

struct Processor {
    target: u16,
}

impl Processor {
    fn event(&self, event: Event) -> Output {
        match event {
            Event::Button(key, down) if key == self.target => Output::Hold(down),
            Event::Disconnect => Output::Error,
            other => Output::Forward(other),
        }
    }
}

#[test]
fn movement_and_wheels_are_unchanged() {
    let processor = Processor { target: 1 };
    for event in [Event::Move(4, -2), Event::Wheel(1, -1)] {
        assert_eq!(processor.event(event.clone()), Output::Forward(event));
    }
}

#[test]
fn unrelated_button_is_unchanged() {
    let processor = Processor { target: 1 };
    let event = Event::Button(2, true);
    assert_eq!(processor.event(event.clone()), Output::Forward(event));
}

#[test]
fn left_button_controls_hold_state() {
    let processor = Processor { target: 1 };
    assert_eq!(processor.event(Event::Button(1, true)), Output::Hold(true));
    assert_eq!(
        processor.event(Event::Button(1, false)),
        Output::Hold(false)
    );
}

#[test]
fn disconnect_is_clean_error() {
    let processor = Processor { target: 1 };
    assert_eq!(processor.event(Event::Disconnect), Output::Error);
}

#[test]
fn unsuitable_sources_are_filtered() {
    assert!(!is_candidate(VIRTUAL_NAME, true, true, false));
    assert!(!is_candidate("Keyboard", false, true, false));
    assert!(!is_candidate("Touchpad", true, true, false));
    assert!(!is_candidate("Tablet", true, true, true));
    assert!(is_candidate("Mouse", true, true, false));
}
