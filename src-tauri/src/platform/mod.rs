#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::agent;
#[cfg(windows)]
pub use windows::agent;

#[cfg(windows)]
pub use windows::executor;

#[cfg(unix)]
pub use unix::{display_local_path, local_system_roots, path_within};
#[cfg(windows)]
pub use windows::paths::{display_local_path, local_system_roots, path_within};
