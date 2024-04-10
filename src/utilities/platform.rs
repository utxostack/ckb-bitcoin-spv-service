//! Enhance the platform environment for continuously running services.

use fdlimit::{raise_fd_limit, Outcome};

pub fn try_raise_fd_limit() {
    match raise_fd_limit() {
        Ok(Outcome::LimitRaised { from, to }) => {
            log::info!("raise file descriptor resource limit from {from} to {to}");
        }
        Ok(Outcome::Unsupported) => {
            log::warn!("raising limit is not supported on this platform");
        }
        Err(err) => {
            log::error!("failed to raise file descriptor resource limit, since {err}");
        }
    }
}
