use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::NamedTempFile;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub fn header(command: &str, env: &str, provider: Option<&str>) {
    let command = command.to_ascii_uppercase();
    match provider {
        Some(provider) => println!("{command} env={env} provider={provider}"),
        None => println!("{command} env={env}"),
    }
}

pub fn section(title: &str) {
    println!("{}", title.to_ascii_uppercase());
}

pub fn step(text: impl Into<String>) -> Step {
    let text = text.into();
    println!("{:<5} {}", "RUN", text);
    Step {
        text,
        started_at: Instant::now(),
        last_heartbeat_at: Instant::now(),
    }
}

pub fn info(text: &str) {
    println!("{:<5} {}", "INFO", text);
}

pub fn ok(text: &str) {
    println!("{:<5} {}", "DONE", text);
}

pub fn warn(text: &str) {
    eprintln!("{:<5} {}", "WARN", text);
}

pub fn next(text: &str) {
    println!("{:<5} {}", "NEXT", text);
}

pub fn detail(label: &str, value: impl std::fmt::Display) {
    println!("{label}: {value}");
}

pub fn blank() {
    println!();
}

pub fn print_failures(title: &str, failures: &[String]) {
    eprintln!("{:<5} {}", "ERROR", title);
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
            println!(
                "{:<5} {} ({})",
                "WAIT",
                waiting_text,
                format_duration(now.duration_since(started_at))
            );
        }

        thread::sleep(Duration::from_secs(1));
    }
}

pub struct Step {
    text: String,
    started_at: Instant,
    last_heartbeat_at: Instant,
}

impl Step {
    pub fn heartbeat(&mut self) {
        self.heartbeat_with(&format!("still {}", self.text));
    }

    pub fn heartbeat_with(&mut self, text: &str) {
        let now = Instant::now();
        if now.duration_since(self.last_heartbeat_at) < HEARTBEAT_INTERVAL {
            return;
        }
        self.last_heartbeat_at = now;
        println!(
            "{:<5} {} ({})",
            "WAIT",
            text,
            format_duration(now.duration_since(self.started_at))
        );
    }

    pub fn done(self, text: &str) {
        println!(
            "{:<5} {} ({})",
            "DONE",
            text,
            format_duration(self.started_at.elapsed())
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
