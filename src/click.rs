use std::io;
use std::time::Duration;
use std::time::Instant;

pub const MIN_CPS: u32 = 1;
pub const DEFAULT_CPS: u32 = 20;
pub const MAX_CPS: u32 = 600;

pub fn valid_cps(cps: u32) -> bool {
    (MIN_CPS..=MAX_CPS).contains(&cps)
}

pub fn cps_error() -> String {
    format!("CPS must be between {MIN_CPS} and {MAX_CPS}.")
}

pub fn cps_interval(cps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(cps))
}

pub fn press_duration(click_period: Duration) -> Duration {
    Duration::from_millis(5).min(click_period / 2)
}

pub trait FrameSink {
    /// Emit one button transition. Each call is one synchronization frame.
    fn emit_button_frame(&mut self, down: bool) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerCommand {
    Hold(bool),
    Shutdown,
}

/// Deadline-driven click scheduler state, independent of evdev and real time.
pub struct Scheduler {
    hold_period: Duration,
    holding: bool,
    button_down: bool,
    next_press: Option<Instant>,
    release_deadline: Option<Instant>,
}

impl Scheduler {
    pub fn new(cps: u32) -> Self {
        Self {
            hold_period: cps_interval(cps),
            holding: false,
            button_down: false,
            next_press: None,
            release_deadline: None,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        match (self.next_press, self.release_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }

    pub fn command<S: FrameSink>(
        &mut self,
        command: SchedulerCommand,
        now: Instant,
        sink: &mut S,
    ) -> io::Result<bool> {
        match command {
            SchedulerCommand::Hold(true) => {
                self.holding = true;
                if self.next_press.is_none() && !self.button_down {
                    self.next_press = Some(now);
                }
                Ok(true)
            }
            SchedulerCommand::Hold(false) => {
                self.holding = false;
                self.next_press = None;
                self.release_if_down(sink)?;
                Ok(true)
            }
            SchedulerCommand::Shutdown => {
                self.holding = false;
                self.next_press = None;
                self.release_if_down(sink)?;
                Ok(false)
            }
        }
    }

    pub fn advance<S: FrameSink>(&mut self, now: Instant, sink: &mut S) -> io::Result<()> {
        loop {
            let Some(deadline) = self.deadline() else {
                return Ok(());
            };
            if deadline > now {
                return Ok(());
            }
            if self.release_deadline == Some(deadline) {
                self.release_if_down(sink)?;
                continue;
            }
            if self.next_press == Some(deadline) {
                self.next_press = None;
                sink.emit_button_frame(true)?;
                self.button_down = true;
                let period = self.hold_period;
                self.release_deadline = Some(deadline + press_duration(period));
                if self.holding {
                    self.next_press = Some(deadline + period);
                }
            }
        }
    }

    fn release_if_down<S: FrameSink>(&mut self, sink: &mut S) -> io::Result<()> {
        self.release_deadline = None;
        if self.button_down {
            sink.emit_button_frame(false)?;
            self.button_down = false;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Frames(Vec<Vec<bool>>);
    impl FrameSink for Frames {
        fn emit_button_frame(&mut self, down: bool) -> io::Result<()> {
            self.0.push(vec![down]);
            Ok(())
        }
    }
    #[test]
    fn interval() {
        assert_eq!(cps_interval(MIN_CPS), Duration::from_secs(1));
        assert_eq!(cps_interval(20), Duration::from_millis(50));
        assert_eq!(press_duration(cps_interval(5)), Duration::from_millis(5));
        assert_eq!(press_duration(cps_interval(20)), Duration::from_millis(5));
        assert_eq!(press_duration(cps_interval(100)), Duration::from_millis(5));
        assert_eq!(cps_interval(MAX_CPS), Duration::from_nanos(1_666_666));
        assert_eq!(
            press_duration(cps_interval(MAX_CPS)),
            Duration::from_nanos(833_333)
        );
    }
    #[test]
    fn one_click_is_two_separate_frames() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(20);
        let mut frames = Frames::default();
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut frames)
            .unwrap();
        scheduler.advance(start, &mut frames).unwrap();
        scheduler
            .advance(start + Duration::from_millis(5), &mut frames)
            .unwrap();
        assert_eq!(frames.0, vec![vec![true], vec![false]]);
        assert!(frames.0.iter().all(|frame| frame.len() == 1));
    }

    #[test]
    fn maximum_rate_keeps_transitions_in_separate_frames() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(MAX_CPS);
        let mut frames = Frames::default();
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut frames)
            .unwrap();
        scheduler.advance(start, &mut frames).unwrap();
        scheduler
            .advance(start + Duration::from_nanos(833_333), &mut frames)
            .unwrap();
        assert_eq!(frames.0, vec![vec![true], vec![false]]);
    }

    #[test]
    fn next_press_deadline_does_not_drift_from_late_release_processing() {
        let start = Instant::now();
        let period = cps_interval(MAX_CPS);
        let mut scheduler = Scheduler::new(MAX_CPS);
        let mut frames = Frames::default();
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut frames)
            .unwrap();
        scheduler.advance(start, &mut frames).unwrap();
        scheduler
            .advance(start + Duration::from_millis(1), &mut frames)
            .unwrap();
        assert_eq!(scheduler.deadline(), Some(start + period));
    }

    #[test]
    fn shutdown_while_pressed_emits_final_release_only_once() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(20);
        let mut frames = Frames::default();
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut frames)
            .unwrap();
        scheduler.advance(start, &mut frames).unwrap();
        assert!(!scheduler
            .command(SchedulerCommand::Shutdown, start, &mut frames)
            .unwrap());
        assert_eq!(frames.0, vec![vec![true], vec![false]]);
        scheduler
            .command(SchedulerCommand::Shutdown, start, &mut frames)
            .unwrap();
        assert_eq!(frames.0.len(), 2);
    }

    #[test]
    fn physical_release_cancels_future_presses() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(20);
        let mut frames = Frames::default();
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut frames)
            .unwrap();
        scheduler.advance(start, &mut frames).unwrap();
        scheduler
            .command(
                SchedulerCommand::Hold(false),
                start + Duration::from_millis(1),
                &mut frames,
            )
            .unwrap();
        scheduler
            .advance(start + Duration::from_secs(1), &mut frames)
            .unwrap();
        assert_eq!(frames.0, vec![vec![true], vec![false]]);
    }

    #[test]
    fn output_errors_are_propagated() {
        struct Failing;
        impl FrameSink for Failing {
            fn emit_button_frame(&mut self, _down: bool) -> io::Result<()> {
                Err(io::Error::other("fake uinput failure"))
            }
        }
        let start = Instant::now();
        let mut scheduler = Scheduler::new(20);
        scheduler
            .command(SchedulerCommand::Hold(true), start, &mut Failing)
            .unwrap();
        assert_eq!(
            scheduler
                .advance(start, &mut Failing)
                .unwrap_err()
                .to_string(),
            "fake uinput failure"
        );
    }
}
