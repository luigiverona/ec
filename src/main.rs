use clap::Parser;
use ec::{
    cli::{self, Action, Cli},
    doctor, input, runtime, setup, Error, Result,
};
use signal_hook::consts::{SIGINT, SIGTERM};
use std::sync::{atomic::AtomicBool, Arc};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if let Some(cps) = runtime::worker_cps()? {
        return worker(cps);
    }

    match Cli::parse().action {
        None => {
            cli::print_help().map_err(|error| Error::Message(error.to_string()))?;
            println!();
            Ok(())
        }
        Some(Action::Start(cps)) => {
            doctor::start_with(doctor::check(), || runtime::start_background(cps))?;
            println!("EC started at {cps} CPS.");
            Ok(())
        }
        Some(Action::Stop) => {
            if runtime::stop()? {
                println!("EC stopped.");
            } else {
                println!("EC is not running.");
            }
            Ok(())
        }
        Some(Action::Status) => status(),
        Some(Action::Setup) => setup::run(),
        Some(Action::Doctor) => {
            if doctor::run() {
                Ok(())
            } else {
                Err(Error::Message("one or more readiness checks failed".into()))
            }
        }
    }
}

fn worker(cps: u32) -> Result<()> {
    let directory = runtime::dir()?;
    let lock = runtime::acquire(&directory)?;
    let stop = signal_flag()?;
    let result = input::run_ready(cps, stop, |device| {
        runtime::write_status(&directory, &runtime::current_identity(cps, device.into())?)
    });
    runtime::clear(&directory);
    drop(lock);
    result.map(|_| ())
}

fn status() -> Result<()> {
    let directory = runtime::dir()?;
    if let Some(status) = runtime::read_status(&directory)? {
        println!(
            "EC is running at {} CPS.\nDevice: {}\nPID: {}",
            status.cps, status.device, status.pid
        );
    } else {
        println!("EC is not running.");
    }
    Ok(())
}

fn signal_flag() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGINT, Arc::clone(&stop))
        .map_err(|error| Error::Message(format!("failed to install Ctrl+C handler: {error}")))?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&stop)).map_err(|error| {
        Error::Message(format!("failed to install termination handler: {error}"))
    })?;
    Ok(stop)
}
