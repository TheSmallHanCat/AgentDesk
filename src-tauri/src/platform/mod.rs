#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::agent;
#[cfg(windows)]
pub use windows::agent;
