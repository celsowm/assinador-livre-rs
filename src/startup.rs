use anyhow::{Context, Result};
use auto_launch::AutoLaunchBuilder;

const AUTOSTART_APP_NAME: &str = "AssinadorLivre";

pub fn set_startup(enabled: bool) -> Result<()> {
    let exe_path = std::env::current_exe().context("Falha ao descobrir caminho do executavel")?;
    let exe_owned = exe_path.to_string_lossy().to_string();
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name(AUTOSTART_APP_NAME)
        .set_app_path(&exe_owned)
        .set_use_launch_agent(true);
    let app = builder.build().context("Falha ao preparar auto-start")?;

    if enabled {
        if !app.is_enabled().context("Falha ao consultar auto-start")? {
            app.enable().context("Falha ao habilitar auto-start")?;
        }
    } else if app.is_enabled().context("Falha ao consultar auto-start")? {
        app.disable().context("Falha ao desabilitar auto-start")?;
    }

    Ok(())
}
