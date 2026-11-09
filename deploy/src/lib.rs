//! Running the logger as a long-lived service: a supervisor that keeps its
//! programs running, and (to come) installing it as a scheduled task or a
//! Windows service, and showing its status.

pub mod legacy;
pub mod log;
pub mod processes;
pub mod supervise;
