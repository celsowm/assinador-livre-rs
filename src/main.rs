#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod logger;
mod signer;
mod tray;
mod ws;

use rfd::{MessageButtons, MessageDialog, MessageLevel};

fn main() {
    if let Err(e) = app::run() {
        eprintln!("Erro: {e:#}");
        MessageDialog::new()
            .set_title("Erro Tecnico")
            .set_description(format!("{e:#}"))
            .set_level(MessageLevel::Error)
            .set_buttons(MessageButtons::Ok)
            .show();
    }
}
