use anyhow::{bail, Context, Result};
use std::sync::mpsc::Sender;
use tray_item::{IconSource, TrayItem};
use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    SignDocument,
    Exit,
}

pub struct TrayHandle {
    _tray: TrayItem,
}

pub fn create_tray(command_tx: Sender<TrayCommand>) -> Result<TrayHandle> {
    let icon = unsafe { LoadIconW(0, IDI_APPLICATION) };
    if icon == 0 {
        bail!("Falha ao carregar icone padrao do Windows");
    }

    let mut tray = TrayItem::new("Assinador Livre", IconSource::RawIcon(icon))
        .context("Falha ao criar icone da bandeja")?;

    let sign_tx = command_tx.clone();
    tray.add_menu_item("Assinar documento", move || {
        let _ = sign_tx.send(TrayCommand::SignDocument);
    })
    .context("Falha ao criar item de menu 'Assinar documento'")?;

    tray.add_menu_item("Sair", move || {
        let _ = command_tx.send(TrayCommand::Exit);
    })
    .context("Falha ao criar item de menu 'Sair'")?;

    Ok(TrayHandle { _tray: tray })
}
