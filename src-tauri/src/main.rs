// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
extern "system" {
    fn AttachConsole(dw_process_id: u32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if localpdf_lib::cli::should_run_cli(&args) {
        #[cfg(windows)]
        unsafe {
            // ATTACH_PARENT_PROCESS = 0xFFFF_FFFF
            let _ = AttachConsole(0xFFFF_FFFF);
        }
        match localpdf_lib::cli::run_cli_args(&args) {
            Ok(msg) => {
                println!("{msg}");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("錯誤：{err}");
                std::process::exit(1);
            }
        }
    }
    localpdf_lib::run()
}
