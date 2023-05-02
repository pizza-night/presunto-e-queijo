#[cfg(not(target_os = "linux"))]
pub use cursive::backends::crossterm::Backend;
#[cfg(target_os = "linux")]
pub use cursive::backends::curses::n::Backend;
