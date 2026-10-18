#![windows_subsystem = "windows"]

fn main() {
    let result = quotarail::desktop::DesktopOptions::parse(std::env::args_os().skip(1))
        .and_then(quotarail::desktop::windows::run);
    if let Err(error) = result {
        quotarail::desktop::windows::show_error(&error);
        std::process::exit(error.exit_code());
    }
}
