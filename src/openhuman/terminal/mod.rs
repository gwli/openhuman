//! Interactive PTY terminal sessions for local shells and SSH.

pub mod rpc;
mod schemas;
mod session;

pub use schemas::{
    all_controller_schemas as all_terminal_controller_schemas,
    all_registered_controllers as all_terminal_registered_controllers,
};
