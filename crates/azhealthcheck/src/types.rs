use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

impl Severity {
    pub fn exit_code(self) -> i32 {
        match self {
            Severity::Ok => 0,
            Severity::Warning => 1,
            Severity::Error => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckOutcome {
    pub name: &'static str,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub findings: Vec<String>,
}

impl CheckOutcome {
    pub fn ok(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            severity: Severity::Ok,
            message: message.into(),
            findings: vec![],
        }
    }

    pub fn warning(name: &'static str, message: impl Into<String>, findings: Vec<String>) -> Self {
        Self {
            name,
            severity: Severity::Warning,
            message: message.into(),
            findings,
        }
    }

    pub fn error(name: &'static str, message: impl Into<String>, findings: Vec<String>) -> Self {
        Self {
            name,
            severity: Severity::Error,
            message: message.into(),
            findings,
        }
    }
}

pub trait Runner: Sync {
    fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<std::process::Output>;
}

pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new(prog).args(args).output()
    }
}

#[cfg(test)]
pub struct FakeRunner {
    pub responses: std::collections::HashMap<String, std::process::Output>,
}

#[cfg(test)]
impl FakeRunner {
    pub fn new() -> Self {
        Self {
            responses: std::collections::HashMap::new(),
        }
    }

    pub fn with(mut self, key: &str, stdout: &str, status: i32) -> Self {
        use std::os::unix::process::ExitStatusExt;
        let mut out = std::process::Output {
            status: std::process::ExitStatus::from_raw((status & 0xff) << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        if status != 0 {
            out.stderr = b"non-zero".to_vec();
        }
        self.responses.insert(key.to_string(), out);
        self
    }
}

#[cfg(test)]
impl Runner for FakeRunner {
    fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
        let key = format!("{prog} {}", args.join(" "));
        self.responses
            .get(&key)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("no fake response for: {key}")))
    }
}
