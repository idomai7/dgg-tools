//! Running the logger as a long-lived service: a supervisor that keeps its
//! programs running, and (to come) installing it as a scheduled task or a
//! Windows service, and showing its status.

pub mod legacy;
pub mod lifeline;
pub mod log;
pub mod paths;
pub mod processes;
pub mod status;
pub mod supervise;
