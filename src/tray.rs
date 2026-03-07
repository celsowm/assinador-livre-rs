use anyhow::{bail, Context, Result};
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};
use tray_item::{IconSource, TrayItem};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    LoadIconW, LoadImageW, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LR_LOADFROMFILE,
};

use crate::logger;

const ICON_FILE_NAME: &str = "icone-assinador-livre.ico";

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    SignDocument,
    OpenPlayground,
    Exit,
}

pub struct TrayHandle {
    _tray: TrayItem,
}

pub fn create_tray(command_tx: Sender<TrayCommand>) -> Result<TrayHandle> {
    let icon = resolve_icon_path()
        .and_then(|path| {
            let loaded = load_icon_from_file(&path);
            if loaded.is_none() {
                logger::warn(format!(
                    "Nao foi possivel carregar icone customizado da bandeja em {}. Usando padrao do Windows.",
                    path.display()
                ));
            }
            loaded
        })
        .unwrap_or_else(|| unsafe { LoadIconW(0, IDI_APPLICATION) });

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

    let playground_tx = command_tx.clone();
    tray.add_menu_item("Abrir playground", move || {
        let _ = playground_tx.send(TrayCommand::OpenPlayground);
    })
    .context("Falha ao criar item de menu 'Abrir playground'")?;

    tray.add_menu_item("Sair", move || {
        let _ = command_tx.send(TrayCommand::Exit);
    })
    .context("Falha ao criar item de menu 'Sair'")?;

    Ok(TrayHandle { _tray: tray })
}

fn resolve_icon_path() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;
    let candidates = [
        exe_dir.join("assets").join(ICON_FILE_NAME),
        exe_dir.parent().map(|p| p.join("assets").join(ICON_FILE_NAME))?,
        exe_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("assets").join(ICON_FILE_NAME))?,
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn load_icon_from_file(path: &Path) -> Option<HICON> {
    let wide_path: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let icon = unsafe {
        LoadImageW(
            0,
            wide_path.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTCOLOR | LR_LOADFROMFILE,
        )
    };

    if icon == 0 {
        return None;
    }

    Some(icon)
}
