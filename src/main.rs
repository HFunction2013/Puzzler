mod bitset;
mod cli;
mod geom;
mod json;
mod model;
mod parse;
mod render;
mod solve;

/// On Windows 10+, enable ANSI escape-code processing in the console so
/// that colour output works in cmd.exe and the legacy conhost.
#[cfg(windows)]
fn enable_ansi_escapes() {
    #![allow(non_snake_case)]
    use std::os::raw::c_void;
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if !handle.is_null() && handle != (-1isize as *mut c_void) {
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    enable_ansi_escapes();
    std::process::exit(cli::run());
}
