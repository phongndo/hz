mod protocol;

#[cfg(unix)]
mod agent_process;
#[cfg(unix)]
mod agent_store;
#[cfg(unix)]
mod client;
#[cfg(unix)]
mod paths;
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod state;

#[cfg(not(unix))]
mod unsupported;

#[cfg(unix)]
pub use client::{
    DaemonSession, agent_log_path, attach, attach_main_session, detach, list_agents,
    send_agent_input, spawn_agent, start, status, stop, stop_agent,
};
#[cfg(unix)]
pub use server::run_foreground;

#[cfg(not(unix))]
pub use unsupported::{
    DaemonSession, agent_log_path, attach, attach_main_session, detach, list_agents,
    run_foreground, send_agent_input, spawn_agent, start, status, stop, stop_agent,
};
