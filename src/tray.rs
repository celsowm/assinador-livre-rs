use anyhow::{Context, Result};
use image::ImageReader;
use std::{
    path::PathBuf,
    sync::mpsc::Sender,
    thread,
};
#[cfg(not(windows))]
use tray_icon::TrayIcon;
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_QUIT,
};

const ICON_FILE_NAME: &str = "icone-assinador-livre.png";

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    SignDocumentQuick,
    SignDocument,
    OpenPlayground,
    Exit,
}

pub struct TrayHandle {
    #[cfg(windows)]
    thread_id: u32,
    #[cfg(not(windows))]
    _tray: Option<TrayIcon>,
    #[cfg(not(windows))]
    _menu: Option<Menu>,
    #[cfg(not(windows))]
    _quick_sign_item: Option<MenuItem>,
    #[cfg(not(windows))]
    _sign_item: Option<MenuItem>,
    #[cfg(not(windows))]
    _playground_item: Option<MenuItem>,
    #[cfg(not(windows))]
    _exit_item: Option<MenuItem>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn create_tray(command_tx: Sender<TrayCommand>) -> Result<TrayHandle> {
    #[cfg(windows)]
    {
        return create_tray_windows(command_tx);
    }

    #[cfg(not(windows))]
    {
        create_tray_polling(command_tx)
    }
}

#[cfg(windows)]
fn create_tray_windows(command_tx: Sender<TrayCommand>) -> Result<TrayHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();

    let worker = thread::Builder::new()
        .name("tray-win32-loop".to_string())
        .spawn(move || {
            let run = || -> Result<()> {
                let menu = Menu::new();
                let quick_sign_item = MenuItem::new("Assinar rapidamente", true, None);
                let sign_item = MenuItem::new("Assinar documento", true, None);
                let playground_item = MenuItem::new("Abrir playground", true, None);
                let exit_item = MenuItem::new("Sair", true, None);

                menu.append(&quick_sign_item)
                    .context("Falha ao criar item de menu 'Assinar rapidamente'")?;
                menu.append(&sign_item)
                    .context("Falha ao criar item de menu 'Assinar documento'")?;
                menu.append(&playground_item)
                    .context("Falha ao criar item de menu 'Abrir playground'")?;
                menu.append(&exit_item)
                    .context("Falha ao criar item de menu 'Sair'")?;

                let icon = load_icon()?;
                let _tray = TrayIconBuilder::new()
                    .with_tooltip("Assinador Livre")
                    .with_menu(Box::new(menu.clone()))
                    .with_icon(icon)
                    .build()
                    .context("Falha ao inicializar bandeja")?;

                let quick_sign_id = quick_sign_item.id().clone();
                let sign_id = sign_item.id().clone();
                let playground_id = playground_item.id().clone();
                let exit_id = exit_item.id().clone();
                let thread_id = unsafe { GetCurrentThreadId() };
                let handler_tx = command_tx.clone();

                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    if event.id == quick_sign_id {
                        let _ = handler_tx.send(TrayCommand::SignDocumentQuick);
                    } else if event.id == sign_id {
                        let _ = handler_tx.send(TrayCommand::SignDocument);
                    } else if event.id == playground_id {
                        let _ = handler_tx.send(TrayCommand::OpenPlayground);
                    } else if event.id == exit_id {
                        let _ = handler_tx.send(TrayCommand::Exit);
                        unsafe {
                            let _ = PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
                        }
                    }
                }));

                let _ = ready_tx.send(Ok(thread_id));

                let mut msg: MSG = unsafe { std::mem::zeroed() };
                loop {
                    let status = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
                    if status <= 0 {
                        break;
                    }
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }

                MenuEvent::set_event_handler::<fn(MenuEvent)>(None);

                // keep menu and items alive for the duration of the loop
                let _ = (menu, quick_sign_item, sign_item, playground_item, exit_item);
                Ok(())
            };

            if let Err(err) = run() {
                let _ = ready_tx.send(Err(err));
            }
        })
        .context("Falha ao iniciar thread da bandeja")?;

    let thread_id = ready_rx
        .recv()
        .context("Falha ao iniciar bandeja (sem resposta da thread)")??;

    Ok(TrayHandle {
        thread_id,
        worker: Some(worker),
    })
}

#[cfg(not(windows))]
fn create_tray_polling(command_tx: Sender<TrayCommand>) -> Result<TrayHandle> {
    let menu = Menu::new();
    let quick_sign_item = MenuItem::new("Assinar rapidamente", true, None);
    let sign_item = MenuItem::new("Assinar documento", true, None);
    let playground_item = MenuItem::new("Abrir playground", true, None);
    let exit_item = MenuItem::new("Sair", true, None);

    menu.append(&quick_sign_item)
        .context("Falha ao criar item de menu 'Assinar rapidamente'")?;
    menu.append(&sign_item)
        .context("Falha ao criar item de menu 'Assinar documento'")?;
    menu.append(&playground_item)
        .context("Falha ao criar item de menu 'Abrir playground'")?;
    menu.append(&exit_item)
        .context("Falha ao criar item de menu 'Sair'")?;

    let icon = load_icon()?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("Assinador Livre")
        .with_menu(Box::new(menu.clone()))
        .with_icon(icon)
        .build()
        .context("Falha ao inicializar bandeja")?;

    let quick_sign_id = quick_sign_item.id().clone();
    let sign_id = sign_item.id().clone();
    let playground_id = playground_item.id().clone();
    let exit_id = exit_item.id().clone();
    let worker_tx = command_tx.clone();

    let worker = thread::Builder::new()
        .name("tray-menu-events".to_string())
        .spawn(move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                if event.id == quick_sign_id {
                    let _ = worker_tx.send(TrayCommand::SignDocumentQuick);
                } else if event.id == sign_id {
                    let _ = worker_tx.send(TrayCommand::SignDocument);
                } else if event.id == playground_id {
                    let _ = worker_tx.send(TrayCommand::OpenPlayground);
                } else if event.id == exit_id {
                    let _ = worker_tx.send(TrayCommand::Exit);
                    break;
                }
            }
        })
        .context("Falha ao iniciar worker de eventos da bandeja")?;

    Ok(TrayHandle {
        _tray: Some(tray),
        _menu: Some(menu),
        _quick_sign_item: Some(quick_sign_item),
        _sign_item: Some(sign_item),
        _playground_item: Some(playground_item),
        _exit_item: Some(exit_item),
        worker: Some(worker),
    })
}

fn load_icon() -> Result<Icon> {
    let image = if let Some(path) = resolve_icon_path() {
        ImageReader::open(&path)
            .with_context(|| format!("Falha ao abrir icone da bandeja em {}", path.display()))?
            .decode()
            .with_context(|| {
                format!(
                    "Falha ao decodificar icone da bandeja em {}",
                    path.display()
                )
            })?
            .into_rgba8()
    } else {
        image::load_from_memory(include_bytes!("../assets/icone-assinador-livre.png"))
            .context("Falha ao decodificar icone embutido da bandeja")?
            .into_rgba8()
    };

    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
        .map_err(|err| anyhow::anyhow!("Falha ao criar icone da bandeja: {err:#}"))
}

fn resolve_icon_path() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;

    let candidates = [
        exe_dir.join("assets").join(ICON_FILE_NAME),
        exe_dir
            .parent()
            .map(|parent| parent.join("assets").join(ICON_FILE_NAME))?,
        exe_dir
            .parent()
            .and_then(|parent| parent.parent())
            .map(|parent| parent.join("assets").join(ICON_FILE_NAME))?,
    ];

    candidates.into_iter().find(|path| path.exists())
}
