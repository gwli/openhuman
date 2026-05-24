use crate::openhuman::config::Config;
use crate::openhuman::terminal::session::{
    manager, CloseRequest, PollOutputRequest, ResizeRequest, StartSessionRequest, WriteRequest,
};
use crate::rpc::RpcOutcome;
use anyhow::Context;

pub async fn start_session(
    request: StartSessionRequest,
) -> Result<RpcOutcome<crate::openhuman::terminal::session::StartSessionResponse>, String> {
    tracing::debug!(
        kind = ?request.kind,
        approved = request.approved,
        "[rpc:terminal_start_session] starting terminal session"
    );
    let config = Config::load_or_init()
        .await
        .map_err(|error| format!("failed to load config: {error}"))?;
    let response = manager()
        .start_session(&config, request)
        .await
        .map_err(|error| format!("failed to start terminal session: {error}"))?;
    tracing::info!(
        session_id = %response.session_id,
        ?response.kind,
        pid = ?response.pid,
        "[rpc:terminal_start_session] terminal session started"
    );
    Ok(RpcOutcome::single_log(
        response,
        "[terminal] PTY session started",
    ))
}

pub async fn write(request: WriteRequest) -> Result<RpcOutcome<serde_json::Value>, String> {
    tracing::trace!(
        session_id = %request.session_id,
        bytes = request.data.len(),
        "[rpc:terminal_write] writing PTY input"
    );
    let response = manager()
        .write(request)
        .map_err(|error| format!("failed to write terminal input: {error}"))?;
    Ok(RpcOutcome::new(
        serde_json::to_value(response).context("serialize terminal write response").map_err(|e| e.to_string())?,
        vec![],
    ))
}

pub async fn poll_output(
    request: PollOutputRequest,
) -> Result<RpcOutcome<serde_json::Value>, String> {
    let response = manager()
        .poll_output(request)
        .map_err(|error| format!("failed to poll terminal output: {error}"))?;
    Ok(RpcOutcome::new(
        serde_json::to_value(response).map_err(|e| e.to_string())?,
        vec![],
    ))
}

pub async fn resize(request: ResizeRequest) -> Result<RpcOutcome<serde_json::Value>, String> {
    tracing::debug!(
        session_id = %request.session_id,
        rows = request.rows,
        cols = request.cols,
        "[rpc:terminal_resize] resizing PTY session"
    );
    let response = manager()
        .resize(request)
        .map_err(|error| format!("failed to resize terminal session: {error}"))?;
    Ok(RpcOutcome::new(
        serde_json::to_value(response).map_err(|e| e.to_string())?,
        vec![],
    ))
}

pub async fn close(request: CloseRequest) -> Result<RpcOutcome<serde_json::Value>, String> {
    tracing::info!(
        session_id = %request.session_id,
        "[rpc:terminal_close] closing PTY session"
    );
    let response = manager()
        .close(request)
        .map_err(|error| format!("failed to close terminal session: {error}"))?;
    Ok(RpcOutcome::new(
        serde_json::to_value(response).map_err(|e| e.to_string())?,
        vec![],
    ))
}

pub async fn list_sessions() -> Result<RpcOutcome<serde_json::Value>, String> {
    let sessions = manager().list_sessions();
    Ok(RpcOutcome::new(
        serde_json::json!({ "sessions": sessions }),
        vec![],
    ))
}
