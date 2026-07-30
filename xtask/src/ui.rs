use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use colored::{Color, Colorize};
use tempfile::NamedTempFile;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

fn tag(label: &str, color: Color, text: impl std::fmt::Display) {
    println!("{} {text}", format!("{label:<5}").color(color));
}

fn etag(label: &str, color: Color, text: impl std::fmt::Display) {
    eprintln!("{} {text}", format!("{label:<5}").color(color));
}

pub fn header(command: &str, env: &str, provider: Option<&str>) {
    let command = command.to_ascii_uppercase();
    match provider {
        Some(provider) => println!("{} env={env} provider={provider}", command.bold()),
        None => println!("{} env={env}", command.bold()),
    }
}

pub fn section(title: &str) {
    println!();
    println!("{}", title.to_ascii_uppercase().bold());
}

pub fn step(text: impl Into<String>) -> Step {
    tag("RUN", Color::Cyan, text.into());
    Step {
        started_at: Instant::now(),
        last_heartbeat_at: Instant::now(),
    }
}

pub fn info(text: &str) {
    tag("INFO", Color::Blue, text);
}

/// Like `info`, but on stderr — for diagnostics emitted by commands that keep
/// stdout machine-readable under `--json`.
pub fn info_stderr(text: &str) {
    etag("INFO", Color::Blue, text);
}

pub fn ok(text: &str) {
    tag("DONE", Color::Green, text);
}

pub fn warn(text: &str) {
    etag("WARN", Color::Yellow, text);
}

pub fn next(text: &str) {
    tag("NEXT", Color::Cyan, text);
}

pub fn detail(label: &str, value: impl std::fmt::Display) {
    println!("{} {value}", format!("{label}:").dimmed());
}

pub fn field(label: &str, value: impl std::fmt::Display) {
    println!("{:<18} {value}", label);
}

pub fn blank() {
    println!();
}

pub fn print_failures(title: &str, failures: &[String]) {
    etag("ERROR", Color::Red, title);
    for failure in failures {
        eprintln!("  - {failure}");
    }
}

pub fn command_failure(command: &str, output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => format!("{command} failed with status {}", output.status),
        (true, false) => format!("{command} failed with status {}\n{stderr}", output.status),
        (false, true) => format!("{command} failed with status {}\n{stdout}", output.status),
        (false, false) => format!(
            "{command} failed with status {}\nstderr:\n{stderr}\nstdout:\n{stdout}",
            output.status
        ),
    }
}

pub fn run_command(command: &mut Command, waiting_text: &str) -> std::io::Result<Output> {
    let stdout_file = NamedTempFile::new()?;
    let stderr_file = NamedTempFile::new()?;
    let stdout_writer = stdout_file.reopen()?;
    let stderr_writer = stderr_file.reopen()?;

    let mut child = command
        .stdout(Stdio::from(stdout_writer))
        .stderr(Stdio::from(stderr_writer))
        .spawn()?;

    let started_at = Instant::now();
    let mut last_heartbeat_at = started_at;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Output {
                status,
                stdout: std::fs::read(stdout_file.path())?,
                stderr: std::fs::read(stderr_file.path())?,
            });
        }

        let now = Instant::now();
        if now.duration_since(last_heartbeat_at) >= HEARTBEAT_INTERVAL {
            last_heartbeat_at = now;
            let elapsed = format_duration(now.duration_since(started_at));
            println!(
                "{} {} ({})",
                format!("{:<5}", "WAIT").dimmed(),
                waiting_text.dimmed(),
                elapsed.dimmed(),
            );
        }

        thread::sleep(Duration::from_secs(1));
    }
}

pub struct Step {
    started_at: Instant,
    last_heartbeat_at: Instant,
}

impl Step {
    pub fn heartbeat_with(&mut self, text: &str) {
        let now = Instant::now();
        if now.duration_since(self.last_heartbeat_at) < HEARTBEAT_INTERVAL {
            return;
        }
        self.last_heartbeat_at = now;
        let elapsed = format_duration(now.duration_since(self.started_at));
        println!(
            "{} {} ({})",
            format!("{:<5}", "WAIT").dimmed(),
            text.dimmed(),
            elapsed.dimmed(),
        );
    }

    pub fn done(self, text: &str) {
        let elapsed = format_duration(self.started_at.elapsed());
        println!(
            "{} {text} ({})",
            format!("{:<5}", "DONE").green(),
            elapsed.dimmed(),
        );
    }
}

fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1_000 {
        format!("{millis}ms")
    } else if duration.as_secs() < 60 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        let seconds = duration.as_secs();
        format!("{}m{}s", seconds / 60, seconds % 60)
    }
}
