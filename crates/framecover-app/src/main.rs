//! FrameCover desktop application (Slint GUI) and command-line interface.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod controller;
mod embedded;
mod error_report;
mod filmstrip;
mod frame_worker;
mod services;
mod settings;
mod timeline;
mod ui;
mod window_fit;
mod workers;

slint::include_modules!();

fn main() {
    // `framecover <command> ...` runs the CLI; no arguments or file paths
    // open the window.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if cli::is_cli_invocation(&args) {
        std::process::exit(cli::main(args));
    }
    ui::run();
}
