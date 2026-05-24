use crate::openhuman::approval::gate::ApprovalGate;
use crate::openhuman::approval::types::{ExecutionOutcome, GateOutcome};
use crate::openhuman::config::Config;
use crate::openhuman::security::{
    get_or_create_workspace_audit_logger, AuditLogger, CommandExecutionLog, SecurityPolicy,
    ToolOperation,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const MAX_BUFFER_CHUNKS: usize = 4_096;
const MAX_WRITE_BYTES: usize = 64 * 1024;
const MAX_POLL_CHUNKS: usize = 512;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const SAFE_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "SHELL",
    "TMPDIR",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
];

static SENSITIVE_KV_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(token|api[_-]?key|password|secret|user[_-]?key|bearer|credential)["']?\s*[:=]\s*(?:"([^"]{8,})"|'([^']{8,})'|([a-zA-Z0-9_\-\.]{8,}))"#)
        .expect("terminal sensitive regex")
});

static MANAGER: OnceLock<TerminalSessionManager> = OnceLock::new();

pub fn manager() -> &'static TerminalSessionManager {
    MANAGER.get_or_init(TerminalSessionManager::default)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StartSessionRequest {
    #[serde(default)]
    pub kind: TerminalKind,
    pub command: Option<String>,
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub ssh: Option<SshSessionRequest>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SshSessionRequest {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub request_tty: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WriteRequest {
    pub session_id: String,
    pub data: String,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PollOutputRequest {
    pub session_id: String,
    #[serde(default)]
    pub after_seq: Option<u64>,
    #[serde(default)]
    pub max_chunks: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResizeRequest {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloseRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    #[default]
    Local,
    Ssh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Starting,
    Running,
    Exited,
    Closed,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartSessionResponse {
    pub session_id: String,
    pub kind: TerminalKind,
    pub status: TerminalStatus,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutputChunk {
    pub seq: u64,
    pub data: String,
    pub timestamp: DateTime<Utc>,
    pub stream: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PollOutputResponse {
    pub session_id: String,
    pub status: TerminalStatus,
    pub chunks: Vec<TerminalOutputChunk>,
    pub next_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteResponse {
    pub session_id: String,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResizeResponse {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloseResponse {
    pub session_id: String,
    pub status: TerminalStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub kind: TerminalKind,
    pub status: TerminalStatus,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct TerminalSessionManager {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
}

impl TerminalSessionManager {
    pub async fn start_session(
        &self,
        config: &Config,
        request: StartSessionRequest,
    ) -> Result<StartSessionResponse> {
        self.cleanup_idle();
        validate_start_request(&request)?;

        let kind = request.kind;
        let rows = normalized_size(request.rows, DEFAULT_ROWS);
        let cols = normalized_size(request.cols, DEFAULT_COLS);
        let command_label = command_label(config, &request)?;
        let approval = approve_session_if_needed(config, &request, &command_label).await?;
        let audit = audit_logger(config);
        let start = Instant::now();

        let result = self.spawn_session(config, request, rows, cols, audit.clone());
        record_approval_execution(approval.request_id.as_deref(), &result);
        let duration_ms = elapsed_ms(start);
        match &result {
            Ok(_response) => {
                emit_audit(
                    audit.as_ref(),
                    &command_label,
                    risk_level(kind),
                    approval.approved,
                    true,
                    true,
                    duration_ms,
                );
            }
            Err(error) => {
                emit_audit(
                    audit.as_ref(),
                    &command_label,
                    risk_level(kind),
                    approval.approved,
                    true,
                    false,
                    duration_ms,
                )
                .with_error_context(error);
            }
        }

        result
    }

    fn spawn_session(
        &self,
        config: &Config,
        request: StartSessionRequest,
        rows: u16,
        cols: u16,
        audit: Arc<AuditLogger>,
    ) -> Result<StartSessionResponse> {
        let kind = request.kind;
        let mut cmd = build_command(config, &request)?;
        let SpawnedPty {
            child,
            master,
            writer,
            mut reader,
        } = spawn_pty_process(&mut cmd, rows, cols).context("failed to spawn PTY child")?;
        let pid = Some(child.id());

        let session_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let session = Arc::new(TerminalSession {
            id: session_id.clone(),
            kind,
            status: Mutex::new(TerminalStatus::Running),
            child: Mutex::new(Some(child)),
            master: Mutex::new(pair.master),
            writer: Mutex::new(Some(writer)),
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            next_seq: Arc::new(AtomicU64::new(1)),
            size: Mutex::new((rows, cols)),
            pid,
            created_at: now,
            updated_at: Mutex::new(now),
            audit,
        });

        spawn_reader_thread(session.clone(), &mut reader);

        self.sessions
            .lock()
            .insert(session_id.clone(), session.clone());
        tracing::info!(
            session_id = %session_id,
            ?kind,
            pid = ?pid,
            rows,
            cols,
            "[terminal] started PTY session"
        );

        Ok(StartSessionResponse {
            session_id,
            kind,
            status: TerminalStatus::Running,
            pid,
            rows,
            cols,
        })
    }

    pub fn write(&self, request: WriteRequest) -> Result<WriteResponse> {
        if request.data.len() > MAX_WRITE_BYTES {
            bail!("terminal write too large; max {MAX_WRITE_BYTES} bytes per call");
        }
        let session = self.get_session(&request.session_id)?;
        session.ensure_running()?;
        let mut writer_guard = session.writer.lock();
        let writer = writer_guard
            .as_mut()
            .ok_or_else(|| anyhow!("terminal session writer is closed"))?;
        writer
            .write_all(request.data.as_bytes())
            .context("failed to write to PTY")?;
        writer.flush().context("failed to flush PTY writer")?;
        session.touch();
        emit_write_audit(&session, request.actor.as_deref(), request.data.len());
        Ok(WriteResponse {
            session_id: request.session_id,
            bytes_written: request.data.len(),
        })
    }

    pub fn poll_output(&self, request: PollOutputRequest) -> Result<PollOutputResponse> {
        let session = self.get_session(&request.session_id)?;
        session.refresh_status();
        session.touch();
        let after_seq = request.after_seq.unwrap_or(0);
        let max_chunks = request.max_chunks.unwrap_or(MAX_POLL_CHUNKS).min(MAX_POLL_CHUNKS);
        let buffer = session.buffer.lock();
        let chunks: Vec<_> = buffer
            .iter()
            .filter(|chunk| chunk.seq > after_seq)
            .take(max_chunks)
            .cloned()
            .collect();
        let next_seq = chunks
            .last()
            .map(|chunk| chunk.seq)
            .unwrap_or(after_seq)
            .max(session.next_seq.load(Ordering::SeqCst).saturating_sub(1));

        Ok(PollOutputResponse {
            session_id: request.session_id,
            status: *session.status.lock(),
            chunks,
            next_seq,
        })
    }

    pub fn resize(&self, request: ResizeRequest) -> Result<ResizeResponse> {
        let session = self.get_session(&request.session_id)?;
        let rows = normalized_size(Some(request.rows), DEFAULT_ROWS);
        let cols = normalized_size(Some(request.cols), DEFAULT_COLS);
        resize_pty(&session.master.lock(), rows, cols).context("failed to resize PTY")?;
        *session.size.lock() = (rows, cols);
        session.touch();
        tracing::debug!(
            session_id = %request.session_id,
            rows,
            cols,
            "[terminal] resized PTY session"
        );
        Ok(ResizeResponse {
            session_id: request.session_id,
            rows,
            cols,
        })
    }

    pub fn close(&self, request: CloseRequest) -> Result<CloseResponse> {
        let session = self.get_session(&request.session_id)?;
        session.close();
        self.sessions.lock().remove(&request.session_id);
        tracing::info!(session_id = %request.session_id, "[terminal] closed PTY session");
        Ok(CloseResponse {
            session_id: request.session_id,
            status: TerminalStatus::Closed,
        })
    }

    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        self.cleanup_idle();
        self.sessions
            .lock()
            .values()
            .map(|session| session.summary())
            .collect()
    }

    fn get_session(&self, session_id: &str) -> Result<Arc<TerminalSession>> {
        self.sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow!("terminal session not found: {session_id}"))
    }

    fn cleanup_idle(&self) {
        let now = Utc::now();
        let mut stale = Vec::new();
        {
            let sessions = self.sessions.lock();
            for (id, session) in sessions.iter() {
                let updated = *session.updated_at.lock();
                if now
                    .signed_duration_since(updated)
                    .to_std()
                    .unwrap_or_default()
                    > IDLE_TIMEOUT
                {
                    stale.push(id.clone());
                }
            }
        }
        if stale.is_empty() {
            return;
        }
        let mut sessions = self.sessions.lock();
        for id in stale {
            if let Some(session) = sessions.remove(&id) {
                tracing::info!(session_id = %id, "[terminal] idle timeout closing session");
                session.close();
            }
        }
    }
}

struct TerminalSession {
    id: String,
    kind: TerminalKind,
    status: Mutex<TerminalStatus>,
    child: Mutex<Option<Child>>,
    master: Mutex<File>,
    writer: Mutex<Option<File>>,
    buffer: Arc<Mutex<VecDeque<TerminalOutputChunk>>>,
    next_seq: Arc<AtomicU64>,
    size: Mutex<(u16, u16)>,
    pid: Option<u32>,
    created_at: DateTime<Utc>,
    updated_at: Mutex<DateTime<Utc>>,
    audit: Arc<AuditLogger>,
}

struct ApprovalResult {
    approved: bool,
    request_id: Option<String>,
}

impl TerminalSession {
    fn ensure_running(&self) -> Result<()> {
        self.refresh_status();
        match *self.status.lock() {
            TerminalStatus::Running | TerminalStatus::Starting => Ok(()),
            other => bail!("terminal session is not running: {other:?}"),
        }
    }

    fn refresh_status(&self) {
        let mut child_guard = self.child.lock();
        if let Some(child) = child_guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    *self.status.lock() = TerminalStatus::Exited;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        session_id = %self.id,
                        error = %error,
                        "[terminal] failed to poll PTY child status"
                    );
                    *self.status.lock() = TerminalStatus::Error;
                }
            }
        }
    }

    fn close(&self) {
        *self.status.lock() = TerminalStatus::Closed;
        let _ = self.writer.lock().take();
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        emit_audit(
            self.audit.as_ref(),
            "terminal close",
            risk_level(self.kind),
            true,
            true,
            true,
            0,
        );
    }

    fn touch(&self) {
        *self.updated_at.lock() = Utc::now();
    }

    fn append_output(&self, data: String) {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let mut buffer = self.buffer.lock();
        buffer.push_back(TerminalOutputChunk {
            seq,
            data,
            timestamp: Utc::now(),
            stream: "pty",
        });
        while buffer.len() > MAX_BUFFER_CHUNKS {
            buffer.pop_front();
        }
        drop(buffer);
        self.touch();
    }

    fn summary(&self) -> SessionSummary {
        self.refresh_status();
        let (rows, cols) = *self.size.lock();
        SessionSummary {
            session_id: self.id.clone(),
            kind: self.kind,
            status: *self.status.lock(),
            pid: self.pid,
            rows,
            cols,
            created_at: self.created_at,
            updated_at: *self.updated_at.lock(),
        }
    }
}

fn spawn_reader_thread(session: Arc<TerminalSession>, reader: &mut File) {
    let mut reader = reader
        .try_clone()
        .expect("failed to clone terminal PTY reader");
    thread::Builder::new()
        .name(format!("openhuman-terminal-{}", session.id))
        .spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        *session.status.lock() = TerminalStatus::Exited;
                        session.append_output("\r\n[openhuman terminal: session exited]\r\n".into());
                        break;
                    }
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        session.append_output(scrub_credentials(&text));
                    }
                    Err(error) => {
                        *session.status.lock() = TerminalStatus::Error;
                        session.append_output(format!(
                            "\r\n[openhuman terminal: read error: {}]\r\n",
                            scrub_credentials(&error.to_string())
                        ));
                        break;
                    }
                }
            }
        })
        .expect("failed to spawn terminal reader thread");
}

fn validate_start_request(request: &StartSessionRequest) -> Result<()> {
    match request.kind {
        TerminalKind::Local => {
            if request.ssh.is_some() {
                bail!("ssh params are only valid for ssh terminal sessions");
            }
            if let Some(command) = request.command.as_deref() {
                reject_control_chars("command", command)?;
            }
            if let Some(shell) = request.shell.as_deref() {
                reject_control_chars("shell", shell)?;
            }
        }
        TerminalKind::Ssh => {
            let ssh = request
                .ssh
                .as_ref()
                .ok_or_else(|| anyhow!("ssh params are required for ssh terminal sessions"))?;
            validate_ssh_request(ssh)?;
        }
    }
    Ok(())
}

fn validate_ssh_request(ssh: &SshSessionRequest) -> Result<()> {
    validate_atom("ssh host", &ssh.host)?;
    if let Some(user) = ssh.user.as_deref() {
        validate_atom("ssh user", user)?;
    }
    for arg in &ssh.extra_args {
        validate_safe_ssh_arg(arg)?;
    }
    Ok(())
}

fn validate_atom(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} cannot be empty");
    }
    reject_control_chars(label, value)?;
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '`' | '$' | '<' | '>'))
    {
        bail!("{label} contains unsafe characters");
    }
    Ok(())
}

fn reject_control_chars(label: &str, value: &str) -> Result<()> {
    if value.chars().any(|ch| ch == '\0' || ch == '\n' || ch == '\r') {
        bail!("{label} contains control characters");
    }
    Ok(())
}

fn validate_safe_ssh_arg(arg: &str) -> Result<()> {
    const SAFE_FLAGS: &[&str] = &[
        "-4", "-6", "-A", "-a", "-C", "-N", "-T", "-t", "-tt", "-v", "-vv", "-vvv",
    ];
    if SAFE_FLAGS.contains(&arg) {
        return Ok(());
    }
    bail!("unsupported ssh extra arg '{arg}'");
}

struct SpawnedPty {
    child: Child,
    master: File,
    writer: File,
    reader: File,
}

#[cfg(unix)]
fn spawn_pty_process(command: &mut Command, rows: u16, cols: u16) -> Result<SpawnedPty> {
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut size,
        )
    };
    if rc != 0 {
        bail!(
            "openpty failed: {}",
            std::io::Error::last_os_error()
        );
    }

    let master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let stdin = slave.try_clone().context("clone PTY slave for stdin")?;
    let stdout = slave.try_clone().context("clone PTY slave for stdout")?;
    let stderr = slave.try_clone().context("clone PTY slave for stderr")?;
    command
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command.spawn().context("spawn PTY command")?;
    drop(slave);

    let reader = master.try_clone().context("clone PTY master for reader")?;
    let writer = master.try_clone().context("clone PTY master for writer")?;
    Ok(SpawnedPty {
        child,
        master,
        writer,
        reader,
    })
}

#[cfg(not(unix))]
fn spawn_pty_process(_command: &mut Command, _rows: u16, _cols: u16) -> Result<SpawnedPty> {
    bail!("interactive PTY sessions are not supported on this platform yet")
}

#[cfg(unix)]
fn resize_pty(master: &File, rows: u16, cols: u16) -> Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    if rc == 0 {
        Ok(())
    } else {
        bail!("ioctl(TIOCSWINSZ) failed: {}", std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn resize_pty(_master: &File, _rows: u16, _cols: u16) -> Result<()> {
    bail!("interactive PTY resize is not supported on this platform yet")
}

fn build_command(config: &Config, request: &StartSessionRequest) -> Result<Command> {
    match request.kind {
        TerminalKind::Local => build_local_command(config, request),
        TerminalKind::Ssh => build_ssh_command(request),
    }
}

fn build_local_command(config: &Config, request: &StartSessionRequest) -> Result<Command> {
    let shell = request
        .shell
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });
    let cwd = resolve_cwd(config, request.cwd.as_deref())?;

    if cfg!(windows) {
        let mut cmd = Command::new(shell);
        configure_command_env(&mut cmd);
        cmd.current_dir(cwd);
        return Ok(cmd);
    }

    let mut cmd = if let Some(command) = request.command.as_deref() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-lc");
        cmd.arg(format!("exec {command}"));
        cmd
    } else {
        Command::new(shell)
    };
    configure_command_env(&mut cmd);
    cmd.current_dir(cwd);
    Ok(cmd)
}

fn build_ssh_command(request: &StartSessionRequest) -> Result<Command> {
    let ssh = request.ssh.as_ref().expect("validated ssh params");
    let mut cmd = Command::new("ssh");
    if let Some(port) = ssh.port {
        cmd.arg("-p");
        cmd.arg(port.to_string());
    }
    if ssh.request_tty {
        cmd.arg("-tt");
    }
    for arg in &ssh.extra_args {
        cmd.arg(arg);
    }
    let target = match ssh.user.as_deref() {
        Some(user) if !user.is_empty() => format!("{user}@{}", ssh.host),
        _ => ssh.host.clone(),
    };
    cmd.arg(target);
    configure_command_env(&mut cmd);
    Ok(cmd)
}

fn configure_command_env(command: &mut Command) {
    command.env_clear();
    for var in SAFE_ENV_VARS {
        if let Ok(value) = std::env::var(var) {
            command.env(var, value);
        }
    }
    command.env("TERM", "xterm-256color");
    command.env("OPENHUMAN_TERMINAL", "1");
}

fn resolve_cwd(config: &Config, cwd: Option<&Path>) -> Result<PathBuf> {
    let base = config.workspace_dir.clone();
    let requested = cwd.map(Path::to_path_buf).unwrap_or(base.clone());
    let candidate = if requested.is_absolute() {
        requested
    } else {
        base.join(requested)
    };
    std::fs::create_dir_all(&base).ok();
    if candidate.exists() {
        let root = std::fs::canonicalize(&base).context("failed to canonicalize workspace dir")?;
        let resolved = std::fs::canonicalize(&candidate).context("failed to canonicalize cwd")?;
        if !resolved.starts_with(&root) {
            bail!("terminal cwd must be inside the OpenHuman workspace");
        }
        Ok(resolved)
    } else {
        bail!("terminal cwd does not exist: {}", candidate.display());
    }
}

fn normalized_size(value: Option<u16>, default: u16) -> u16 {
    value.filter(|v| *v > 0).unwrap_or(default).clamp(1, 500)
}

fn command_label(config: &Config, request: &StartSessionRequest) -> Result<String> {
    Ok(match request.kind {
        TerminalKind::Local => {
            let cwd = resolve_cwd(config, request.cwd.as_deref())?;
            if let Some(command) = request.command.as_deref() {
                format!("local:{}:{}", cwd.display(), truncate(command, 160))
            } else {
                format!("local-shell:{}", cwd.display())
            }
        }
        TerminalKind::Ssh => {
            let ssh = request.ssh.as_ref().expect("validated ssh params");
            let target = match ssh.user.as_deref() {
                Some(user) if !user.is_empty() => format!("{user}@{}", ssh.host),
                _ => ssh.host.clone(),
            };
            format!("ssh:{target}:{}", ssh.port.unwrap_or(22))
        }
    })
}

async fn approve_session_if_needed(
    config: &Config,
    request: &StartSessionRequest,
    command_label: &str,
) -> Result<ApprovalResult> {
    let policy = SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);
    policy
        .enforce_tool_operation(ToolOperation::Act, "terminal.start_session")
        .map_err(|err| anyhow!(err))?;

    if let (TerminalKind::Local, Some(command)) = (request.kind, request.command.as_deref()) {
        policy
            .validate_command_execution(command, request.approved)
            .map_err(|err| anyhow!(err))?;
    }

    if request.kind != TerminalKind::Ssh && request.command.is_none() {
        return Ok(ApprovalResult {
            approved: true,
            request_id: None,
        });
    }

    if request.approved {
        return Ok(ApprovalResult {
            approved: true,
            request_id: None,
        });
    }

    if let Some(gate) = ApprovalGate::try_global() {
        let (outcome, request_id) = gate
            .intercept_audited(
                "terminal_session",
                command_label,
                json!({
                    "kind": request.kind,
                    "command": command_label,
                    "actor": request.actor,
                }),
            )
            .await;
        match outcome {
            GateOutcome::Allow => {
                Ok(ApprovalResult {
                    approved: true,
                    request_id,
                })
            }
            GateOutcome::Deny { reason } => bail!(reason),
        }
    } else {
        bail!("terminal session requires explicit approval")
    }
}

fn record_approval_execution(request_id: Option<&str>, result: &Result<StartSessionResponse>) {
    let Some(request_id) = request_id else {
        return;
    };
    let Some(gate) = ApprovalGate::try_global() else {
        return;
    };
    match result {
        Ok(_) => gate.record_execution(request_id, ExecutionOutcome::Success, None),
        Err(error) => gate.record_execution(
            request_id,
            ExecutionOutcome::Failure,
            Some(&scrub_credentials(&error.to_string())),
        ),
    }
}

fn audit_logger(config: &Config) -> Arc<AuditLogger> {
    let openhuman_dir = config
        .config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config.workspace_dir.clone());
    get_or_create_workspace_audit_logger(config.channels_config.security.audit.clone(), openhuman_dir)
        .unwrap_or_else(|error| {
            tracing::warn!(
                error = %error,
                "[terminal] failed to create audit logger, using disabled logger"
            );
            AuditLogger::disabled()
        })
}

fn risk_level(kind: TerminalKind) -> &'static str {
    match kind {
        TerminalKind::Local => "medium",
        TerminalKind::Ssh => "high",
    }
}

fn emit_write_audit(session: &TerminalSession, actor: Option<&str>, bytes: usize) {
    let actor = actor.unwrap_or("user");
    emit_audit(
        session.audit.as_ref(),
        &format!("terminal write by {actor}: {bytes} bytes"),
        risk_level(session.kind),
        true,
        true,
        true,
        0,
    );
}

fn emit_audit(
    audit: &AuditLogger,
    command: &str,
    risk_level: &str,
    approved: bool,
    allowed: bool,
    success: bool,
    duration_ms: u64,
) -> AuditEmitResult {
    let safe_command = scrub_credentials(command);
    if let Err(error) = audit.log_command_event(CommandExecutionLog {
        channel: "terminal",
        command: &safe_command,
        risk_level,
        approved,
        allowed,
        success,
        duration_ms,
    }) {
        tracing::warn!(error = %error, "[terminal] failed to write audit event");
    }
    AuditEmitResult
}

struct AuditEmitResult;

impl AuditEmitResult {
    fn with_error_context(self, error: &anyhow::Error) {
        tracing::debug!(error = %error, "[terminal] audited failed terminal operation");
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        format!("{}...", &input[..crate::openhuman::util::floor_char_boundary(input, max)])
    }
}

fn scrub_credentials(input: &str) -> String {
    SENSITIVE_KV_REGEX
        .replace_all(input, |caps: &regex::Captures| {
            let key = &caps[1];
            let value = caps
                .get(2)
                .or(caps.get(3))
                .or(caps.get(4))
                .map(|m| m.as_str())
                .unwrap_or("");
            let prefix: String = value.chars().take(4).collect();
            if caps[0].contains('=') {
                format!("{key}={prefix}*[REDACTED]")
            } else {
                format!("{key}: {prefix}*[REDACTED]")
            }
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn config_for(tmp: &TempDir) -> Config {
        Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        }
    }

    #[test]
    fn validates_safe_ssh_target() {
        let ssh = SshSessionRequest {
            host: "example.com".into(),
            user: Some("alice".into()),
            port: Some(22),
            request_tty: true,
            extra_args: vec!["-C".into()],
        };
        assert!(validate_ssh_request(&ssh).is_ok());
    }

    #[test]
    fn rejects_ssh_shell_metacharacters() {
        let ssh = SshSessionRequest {
            host: "example.com;rm -rf /".into(),
            user: None,
            port: None,
            request_tty: false,
            extra_args: vec![],
        };
        assert!(validate_ssh_request(&ssh).is_err());
    }

    #[test]
    fn rejects_unsupported_ssh_extra_args() {
        assert!(validate_safe_ssh_arg("-oProxyCommand=sh").is_err());
    }

    #[test]
    fn resolve_cwd_stays_inside_workspace() {
        let tmp = TempDir::new().unwrap();
        let cfg = config_for(&tmp);
        std::fs::create_dir_all(cfg.workspace_dir.join("sub")).unwrap();
        let cwd = resolve_cwd(&cfg, Some(Path::new("sub"))).unwrap();
        assert!(cwd.ends_with("sub"));
    }
}
