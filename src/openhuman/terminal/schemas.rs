use serde_json::{Map, Value};

use crate::core::all::{ControllerFuture, RegisteredController};
use crate::core::{ControllerSchema, FieldSchema, TypeSchema};

const FUNCTIONS: &[&str] = &[
    "start_session",
    "write",
    "poll_output",
    "resize",
    "close",
    "list_sessions",
];

pub fn all_controller_schemas() -> Vec<ControllerSchema> {
    FUNCTIONS.iter().map(|function| schemas(function)).collect()
}

pub fn all_registered_controllers() -> Vec<RegisteredController> {
    vec![
        RegisteredController {
            schema: schemas("start_session"),
            handler: handle_start_session,
        },
        RegisteredController {
            schema: schemas("write"),
            handler: handle_write,
        },
        RegisteredController {
            schema: schemas("poll_output"),
            handler: handle_poll_output,
        },
        RegisteredController {
            schema: schemas("resize"),
            handler: handle_resize,
        },
        RegisteredController {
            schema: schemas("close"),
            handler: handle_close,
        },
        RegisteredController {
            schema: schemas("list_sessions"),
            handler: handle_list_sessions,
        },
    ]
}

pub fn schemas(function: &str) -> ControllerSchema {
    match function {
        "start_session" => ControllerSchema {
            namespace: "terminal",
            function: "start_session",
            description: "Start an interactive PTY terminal session for a local shell or SSH target.",
            inputs: vec![
                optional_string("kind", "Terminal kind: local or ssh. Defaults to local."),
                optional_string("command", "Optional local command to execute inside the PTY."),
                optional_string("shell", "Optional local shell executable."),
                optional_string("cwd", "Workspace-relative or workspace-contained local cwd."),
                optional_u64("rows", "Initial terminal row count."),
                optional_u64("cols", "Initial terminal column count."),
                optional_json("ssh", "SSH params: host, user, port, request_tty, extra_args."),
                optional_bool("approved", "Explicit user approval for high-risk sessions."),
                optional_string("actor", "Human-readable caller label for audit logs."),
            ],
            outputs: vec![required_json(
                "session",
                "Started terminal session metadata including session_id.",
            )],
        },
        "write" => ControllerSchema {
            namespace: "terminal",
            function: "write",
            description: "Write input bytes to an interactive PTY terminal session.",
            inputs: vec![
                required_string("session_id", "Terminal session id."),
                required_string("data", "Raw input to write to the PTY."),
                optional_string("actor", "Human-readable caller label for audit logs."),
            ],
            outputs: vec![required_json("result", "Write result and byte count.")],
        },
        "poll_output" => ControllerSchema {
            namespace: "terminal",
            function: "poll_output",
            description: "Poll buffered output chunks from an interactive PTY terminal session.",
            inputs: vec![
                required_string("session_id", "Terminal session id."),
                optional_u64("after_seq", "Only return chunks after this sequence id."),
                optional_u64("max_chunks", "Maximum chunks to return, capped by the core."),
            ],
            outputs: vec![required_json("output", "Terminal output chunks and next sequence id.")],
        },
        "resize" => ControllerSchema {
            namespace: "terminal",
            function: "resize",
            description: "Resize an interactive PTY terminal session.",
            inputs: vec![
                required_string("session_id", "Terminal session id."),
                required_u64("rows", "Terminal row count."),
                required_u64("cols", "Terminal column count."),
            ],
            outputs: vec![required_json("result", "Applied terminal size.")],
        },
        "close" => ControllerSchema {
            namespace: "terminal",
            function: "close",
            description: "Close and remove an interactive PTY terminal session.",
            inputs: vec![required_string("session_id", "Terminal session id.")],
            outputs: vec![required_json("result", "Closed terminal session status.")],
        },
        "list_sessions" => ControllerSchema {
            namespace: "terminal",
            function: "list_sessions",
            description: "List active interactive PTY terminal sessions.",
            inputs: vec![],
            outputs: vec![required_json("sessions", "Active terminal session summaries.")],
        },
        _ => ControllerSchema {
            namespace: "terminal",
            function: "unknown",
            description: "Unknown terminal controller.",
            inputs: vec![],
            outputs: vec![required_string("error", "Lookup error details.")],
        },
    }
}

fn handle_start_session(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload: crate::openhuman::terminal::session::StartSessionRequest =
            serde_json::from_value(Value::Object(params))
                .map_err(|e| format!("invalid terminal start_session params: {e}"))?;
        crate::openhuman::terminal::rpc::start_session(payload)
            .await?
            .into_cli_compatible_json()
    })
}

fn handle_write(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload: crate::openhuman::terminal::session::WriteRequest =
            serde_json::from_value(Value::Object(params))
                .map_err(|e| format!("invalid terminal write params: {e}"))?;
        crate::openhuman::terminal::rpc::write(payload)
            .await?
            .into_cli_compatible_json()
    })
}

fn handle_poll_output(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload: crate::openhuman::terminal::session::PollOutputRequest =
            serde_json::from_value(Value::Object(params))
                .map_err(|e| format!("invalid terminal poll_output params: {e}"))?;
        crate::openhuman::terminal::rpc::poll_output(payload)
            .await?
            .into_cli_compatible_json()
    })
}

fn handle_resize(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload: crate::openhuman::terminal::session::ResizeRequest =
            serde_json::from_value(Value::Object(params))
                .map_err(|e| format!("invalid terminal resize params: {e}"))?;
        crate::openhuman::terminal::rpc::resize(payload)
            .await?
            .into_cli_compatible_json()
    })
}

fn handle_close(params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        let payload: crate::openhuman::terminal::session::CloseRequest =
            serde_json::from_value(Value::Object(params))
                .map_err(|e| format!("invalid terminal close params: {e}"))?;
        crate::openhuman::terminal::rpc::close(payload)
            .await?
            .into_cli_compatible_json()
    })
}

fn handle_list_sessions(_params: Map<String, Value>) -> ControllerFuture {
    Box::pin(async move {
        crate::openhuman::terminal::rpc::list_sessions()
            .await?
            .into_cli_compatible_json()
    })
}

fn required_string(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::String,
        comment,
        required: true,
    }
}

fn optional_string(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::Option(Box::new(TypeSchema::String)),
        comment,
        required: false,
    }
}

fn required_u64(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::U64,
        comment,
        required: true,
    }
}

fn optional_u64(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::Option(Box::new(TypeSchema::U64)),
        comment,
        required: false,
    }
}

fn optional_bool(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::Option(Box::new(TypeSchema::Bool)),
        comment,
        required: false,
    }
}

fn required_json(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::Json,
        comment,
        required: true,
    }
}

fn optional_json(name: &'static str, comment: &'static str) -> FieldSchema {
    FieldSchema {
        name,
        ty: TypeSchema::Option(Box::new(TypeSchema::Json)),
        comment,
        required: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_terminal_schemas_have_handlers() {
        let schemas = all_controller_schemas();
        let handlers = all_registered_controllers();
        assert_eq!(schemas.len(), handlers.len());
        let functions: Vec<_> = schemas.iter().map(|schema| schema.function).collect();
        assert_eq!(functions, FUNCTIONS);
    }

    #[test]
    fn start_session_schema_includes_ssh_params() {
        let schema = schemas("start_session");
        assert_eq!(schema.namespace, "terminal");
        assert!(schema.inputs.iter().any(|field| field.name == "ssh"));
        assert!(schema.inputs.iter().any(|field| field.name == "approved"));
    }
}
