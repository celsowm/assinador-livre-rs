use crate::{
    contracts::{
        CertSelectionRequest, CertificateSummary, VisibleSignaturePlacement,
        VisibleSignatureRequest, VisibleSignatureStyle, VisibleSignatureTimezone,
    },
    logger,
};
use anyhow::{Context, Result, anyhow};
use image::{Rgba, RgbaImage};
use pdfium_render::prelude::*;
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::{cell::RefCell, env, path::Path, path::PathBuf, rc::Rc};

slint::include_modules!();

pub struct CertDialogInput {
    pub candidates: Vec<CertificateSummary>,
    pub preselected_position: usize,
    pub preview_pdf: Option<PathBuf>,
}

pub struct CertDialogOutput {
    pub cert_selection: Option<CertSelectionRequest>,
    pub visible_signature: Option<VisibleSignatureRequest>,
}

struct PlacementOption {
    ui_label: &'static str,
    value: VisibleSignaturePlacement,
    vertical: bool,
}

const PDFIUM_OVERRIDE_ENV: &str = "ASSINADOR_PDFIUM_PATH";
const PREVIEW_FALLBACK_HINT: &str =
    "A assinatura visivel continua disponivel via posicao predefinida.";

const PLACEMENTS: &[PlacementOption] = &[
    PlacementOption {
        ui_label: "Superior esquerda (horizontal)",
        value: VisibleSignaturePlacement::TopLeftHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Superior esquerda (vertical)",
        value: VisibleSignaturePlacement::TopLeftVertical,
        vertical: true,
    },
    PlacementOption {
        ui_label: "Superior direita (horizontal)",
        value: VisibleSignaturePlacement::TopRightHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Superior direita (vertical)",
        value: VisibleSignaturePlacement::TopRightVertical,
        vertical: true,
    },
    PlacementOption {
        ui_label: "Inferior esquerda (horizontal)",
        value: VisibleSignaturePlacement::BottomLeftHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Inferior esquerda (vertical)",
        value: VisibleSignaturePlacement::BottomLeftVertical,
        vertical: true,
    },
    PlacementOption {
        ui_label: "Inferior direita (horizontal)",
        value: VisibleSignaturePlacement::BottomRightHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Inferior direita (vertical)",
        value: VisibleSignaturePlacement::BottomRightVertical,
        vertical: true,
    },
    PlacementOption {
        ui_label: "Inferior centro (horizontal)",
        value: VisibleSignaturePlacement::BottomCenterHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Inferior centro (vertical)",
        value: VisibleSignaturePlacement::BottomCenterVertical,
        vertical: true,
    },
    PlacementOption {
        ui_label: "Centro (horizontal)",
        value: VisibleSignaturePlacement::CenterHorizontal,
        vertical: false,
    },
    PlacementOption {
        ui_label: "Centro (vertical)",
        value: VisibleSignaturePlacement::CenterVertical,
        vertical: true,
    },
];

#[derive(Clone, Copy)]
struct DragState {
    active: bool,
    offset_x: f32,
    offset_y: f32,
}

impl DragState {
    fn new() -> Self {
        Self {
            active: false,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

pub fn choose_certificate_and_visible_signature(
    input: CertDialogInput,
) -> Result<Option<CertDialogOutput>> {
    if input.candidates.is_empty() {
        return Ok(None);
    }

    let ui = CertDialog::new().context("Falha ao criar janela de selecao de certificado")?;
    let cert_items = input
        .candidates
        .iter()
        .map(|cert| {
            let token = if cert.is_hardware_token {
                "A3/token"
            } else {
                "software"
            };
            let provider = if cert.provider_name.is_empty() {
                "(sem provider)"
            } else {
                cert.provider_name.as_str()
            };
            let subject = if cert.subject.is_empty() {
                "(sem subject)"
            } else {
                cert.subject.as_str()
            };
            format!("[{}] {} | {} | {}", cert.index, subject, token, provider)
        })
        .collect::<Vec<_>>();
    let placement_items = PLACEMENTS
        .iter()
        .map(|entry| entry.ui_label.to_string())
        .collect::<Vec<_>>();

    let cert_items = cert_items
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    let placement_items = placement_items
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>();

    ui.set_certificate_items(ModelRc::new(VecModel::from(cert_items)));
    ui.set_placement_items(ModelRc::new(VecModel::from(placement_items)));

    let preselected = input
        .preselected_position
        .min(input.candidates.len().saturating_sub(1)) as i32;
    ui.set_selected_certificate(preselected);
    ui.set_use_auto(false);
    ui.set_visible_signature(false);
    ui.set_manual_mode(false);
    ui.set_placement_index(default_placement_index() as i32);

    let preview = load_preview(input.preview_pdf.as_deref());
    ui.set_preview_image(preview.image.clone());
    ui.set_preview_available(preview.available);
    ui.set_preview_status(preview.status.into());

    let initial_rect = default_rect_for_placement(default_placement_index());
    ui.set_rect_x_norm(initial_rect[0]);
    ui.set_rect_y_norm(initial_rect[1]);
    ui.set_rect_w_norm(initial_rect[2]);
    ui.set_rect_h_norm(initial_rect[3]);

    let rect_drag = Rc::new(RefCell::new(DragState::new()));
    let pointer_ui = ui.as_weak();
    let pointer_drag = rect_drag.clone();
    ui.on_preview_pointer_down(move |x, y| {
        if let Some(dialog) = pointer_ui.upgrade() {
            let rx = dialog.get_rect_x_norm();
            let ry = dialog.get_rect_y_norm();
            let rw = dialog.get_rect_w_norm();
            let rh = dialog.get_rect_h_norm();
            let inside = x >= rx && x <= rx + rw && y >= ry && y <= ry + rh;
            if inside {
                let mut drag = pointer_drag.borrow_mut();
                drag.active = true;
                drag.offset_x = x - rx;
                drag.offset_y = y - ry;
            }
        }
    });

    let move_ui = ui.as_weak();
    let move_drag = rect_drag.clone();
    ui.on_preview_pointer_move(move |x, y| {
        let Some(dialog) = move_ui.upgrade() else {
            return;
        };
        if !move_drag.borrow().active {
            return;
        }

        let drag = move_drag.borrow();
        let w = dialog.get_rect_w_norm();
        let h = dialog.get_rect_h_norm();
        let nx = clamp(x - drag.offset_x, 0.0, 1.0 - w);
        let ny = clamp(y - drag.offset_y, 0.0, 1.0 - h);
        drop(drag);

        dialog.set_rect_x_norm(nx);
        dialog.set_rect_y_norm(ny);
    });

    let up_drag = rect_drag.clone();
    ui.on_preview_pointer_up(move || {
        up_drag.borrow_mut().active = false;
    });

    let output_slot: Rc<RefCell<Option<Option<CertDialogOutput>>>> = Rc::new(RefCell::new(None));

    let accept_slot = output_slot.clone();
    let accept_ui = ui.as_weak();
    let candidates = input.candidates;
    ui.on_accept(move || {
        let Some(dialog) = accept_ui.upgrade() else {
            return;
        };

        let selected = dialog.get_selected_certificate().max(0) as usize;
        if selected >= candidates.len() {
            dialog.hide().ok();
            *accept_slot.borrow_mut() = Some(Some(CertDialogOutput {
                cert_selection: None,
                visible_signature: None,
            }));
            return;
        }

        let cert_selection = if dialog.get_use_auto() {
            None
        } else {
            let cert = &candidates[selected];
            Some(CertSelectionRequest {
                thumbprint: Some(cert.thumbprint.clone()),
                index: Some(cert.index),
            })
        };

        let placement_idx = dialog.get_placement_index().max(0) as usize;
        let placement = PLACEMENTS
            .get(placement_idx)
            .map(|entry| entry.value)
            .unwrap_or(VisibleSignaturePlacement::BottomCenterHorizontal);

        let visible_signature = if dialog.get_visible_signature() {
            let custom_rect = if dialog.get_manual_mode() && dialog.get_preview_available() {
                Some(normalized_rect_to_pdf_rect(
                    dialog.get_rect_x_norm(),
                    dialog.get_rect_y_norm(),
                    dialog.get_rect_w_norm(),
                    dialog.get_rect_h_norm(),
                ))
            } else {
                None
            };

            Some(VisibleSignatureRequest {
                placement,
                custom_rect,
                style: VisibleSignatureStyle::Default,
                timezone: VisibleSignatureTimezone::Local,
            })
        } else {
            None
        };

        *accept_slot.borrow_mut() = Some(Some(CertDialogOutput {
            cert_selection,
            visible_signature,
        }));
        dialog.hide().ok();
    });

    let reject_slot = output_slot.clone();
    let reject_ui = ui.as_weak();
    ui.on_reject(move || {
        if let Some(dialog) = reject_ui.upgrade() {
            *reject_slot.borrow_mut() = Some(None);
            dialog.hide().ok();
        }
    });

    ui.run()
        .context("Falha ao executar janela de selecao de certificado")?;

    output_slot
        .borrow_mut()
        .take()
        .ok_or_else(|| anyhow::anyhow!("Janela encerrada sem retorno"))
}

struct PreviewData {
    image: Image,
    available: bool,
    status: String,
}

fn load_preview(preview_pdf: Option<&Path>) -> PreviewData {
    let default_image = placeholder_preview(900, 1200);

    let Some(path) = preview_pdf else {
        return PreviewData {
            image: default_image,
            available: false,
            status: preview_unavailable_status("nenhum PDF selecionado"),
        };
    };

    if !path.exists() {
        return PreviewData {
            image: default_image,
            available: false,
            status: preview_unavailable_status("arquivo de PDF nao encontrado"),
        };
    }

    match render_preview_with_pdfium(path) {
        Ok(image) => PreviewData {
            image,
            available: true,
            status: "Preview ativo. Arraste o retangulo para posicionar.".to_string(),
        },
        Err(err) => {
            logger::warn(format!(
                "Preview PDF indisponivel; fallback sem preview ({})",
                err
            ));
            PreviewData {
                image: default_image,
                available: false,
                status: preview_unavailable_status(&preview_error_summary(&err)),
            }
        }
    }
}

fn render_preview_with_pdfium(path: &Path) -> Result<Image> {
    let pdfium = bind_pdfium_for_preview()?;

    let doc = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("Falha ao carregar PDF para preview: {}", path.display()))?;
    let page = doc
        .pages()
        .get(0)
        .context("PDF nao possui primeira pagina para preview")?;

    let render = page
        .render_with_config(&PdfRenderConfig::new().set_target_width(900))
        .context("Falha ao renderizar primeira pagina para preview")?;
    let image = render.as_image();
    let rgba = image.to_rgba8();
    Ok(rgba_to_slint_image(&rgba))
}

fn bind_pdfium_for_preview() -> Result<Pdfium> {
    let candidates = pdfium_candidates();
    let mut attempts = Vec::<String>::new();

    for candidate in &candidates {
        if !candidate.path.exists() {
            attempts.push(format!(
                "{} -> {} (nao encontrado)",
                candidate.source,
                candidate.path.display()
            ));
            continue;
        }

        match Pdfium::bind_to_library(&candidate.path) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(err) => attempts.push(format!(
                "{} -> {} ({})",
                candidate.source,
                candidate.path.display(),
                err
            )),
        }
    }

    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(err) => {
            attempts.push(format!("biblioteca do sistema ({})", err));
            Err(anyhow!(
                "biblioteca PDFium indisponivel; tentativas: {}",
                attempts.join(" | ")
            ))
        }
    }
}

#[derive(Clone, Debug)]
struct PdfiumCandidate {
    source: String,
    path: PathBuf,
}

fn pdfium_candidates() -> Vec<PdfiumCandidate> {
    let mut out = Vec::<PdfiumCandidate>::new();

    if let Ok(raw) = env::var(PDFIUM_OVERRIDE_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let resolved = resolve_pdfium_override_path(PathBuf::from(trimmed));
            push_unique_pdfium_candidate(
                &mut out,
                format!("variavel de ambiente {}", PDFIUM_OVERRIDE_ENV),
                resolved,
            );
        }
    }

    for path in default_pdfium_candidate_paths() {
        push_unique_pdfium_candidate(&mut out, "pacote/local".to_string(), path);
    }

    out
}

fn default_pdfium_candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::<PathBuf>::new();
    let lib = pdfium_library_filename();

    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            push_unique_path(&mut out, exe_dir.join(lib));
            push_unique_path(&mut out, exe_dir.join("pdfium").join(lib));
            push_unique_path(&mut out, exe_dir.join("lib").join(lib));

            if let Some(parent) = exe_dir.parent() {
                push_unique_path(&mut out, parent.join(lib));
                push_unique_path(&mut out, parent.join("pdfium").join(lib));
                push_unique_path(&mut out, parent.join("lib").join(lib));
                push_unique_path(
                    &mut out,
                    parent
                        .join("third_party")
                        .join("pdfium")
                        .join(pdfium_platform_dir())
                        .join(lib),
                );

                if let Some(grand_parent) = parent.parent() {
                    push_unique_path(&mut out, grand_parent.join(lib));
                    push_unique_path(
                        &mut out,
                        grand_parent
                            .join("third_party")
                            .join("pdfium")
                            .join(pdfium_platform_dir())
                            .join(lib),
                    );
                }
            }

            if cfg!(target_os = "macos") {
                if let Some(contents_dir) = exe_dir.parent() {
                    push_unique_path(&mut out, contents_dir.join("Frameworks").join(lib));
                    if let Some(app_dir) = contents_dir.parent() {
                        push_unique_path(&mut out, app_dir.join("Frameworks").join(lib));
                    }
                }
            }
        }
    }

    push_unique_path(&mut out, PathBuf::from(".").join(lib));
    push_unique_path(&mut out, repo_pdfium_library_path());

    out
}

fn resolve_pdfium_override_path(path: PathBuf) -> PathBuf {
    if path.is_dir() {
        return path.join(pdfium_library_filename());
    }
    path
}

fn repo_pdfium_library_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("third_party")
        .join("pdfium")
        .join(pdfium_platform_dir())
        .join(pdfium_library_filename())
}

fn pdfium_platform_dir() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else {
        "unsupported"
    }
}

fn pdfium_library_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if paths.iter().any(|existing| existing == &path) {
        return;
    }
    paths.push(path);
}

fn push_unique_pdfium_candidate(
    candidates: &mut Vec<PdfiumCandidate>,
    source: String,
    path: PathBuf,
) {
    if candidates.iter().any(|existing| existing.path == path) {
        return;
    }
    candidates.push(PdfiumCandidate { source, path });
}

fn preview_error_summary(err: &anyhow::Error) -> String {
    let detail = format!("{err:#}");
    let first_line = detail
        .lines()
        .next()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "falha ao inicializar preview".to_string());

    if first_line.len() > 220 {
        format!("{}...", &first_line[..220])
    } else {
        first_line
    }
}

fn preview_unavailable_status(reason: &str) -> String {
    format!(
        "Preview indisponivel: {}. {}",
        reason, PREVIEW_FALLBACK_HINT
    )
}

fn rgba_to_slint_image(image: &RgbaImage) -> Image {
    let (width, height) = image.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(image.as_raw());
    Image::from_rgba8(buffer)
}

fn placeholder_preview(width: u32, height: u32) -> Image {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([243, 244, 246, 255]));
    for y in 0..height {
        let x = width / 2;
        image.put_pixel(x, y, Rgba([229, 231, 235, 255]));
    }
    for x in 0..width {
        let y = height / 2;
        image.put_pixel(x, y, Rgba([229, 231, 235, 255]));
    }
    rgba_to_slint_image(&image)
}

fn default_placement_index() -> usize {
    PLACEMENTS
        .iter()
        .position(|entry| entry.value == VisibleSignaturePlacement::BottomCenterHorizontal)
        .unwrap_or(0)
}

fn default_rect_for_placement(index: usize) -> [f32; 4] {
    let vertical = PLACEMENTS
        .get(index)
        .map(|entry| entry.vertical)
        .unwrap_or(false);
    let (w, h) = if vertical { (0.12, 0.24) } else { (0.24, 0.10) };
    let x = (1.0 - w) / 2.0;
    let y = (1.0 - h) / 2.0;
    [x, y, w, h]
}

fn normalized_rect_to_pdf_rect(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    let x0 = clamp(x, 0.0, 1.0);
    let y0 = clamp(y, 0.0, 1.0);
    let x1 = clamp(x + w, 0.0, 1.0);
    let y1 = clamp(y + h, 0.0, 1.0);

    let y_top = 1.0 - y0;
    let y_bottom = 1.0 - y1;

    [x0, y_bottom, x1, y_top]
}

fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_normalized_rect_to_pdf_coords() {
        let rect = normalized_rect_to_pdf_rect(0.10, 0.20, 0.30, 0.40);
        assert!((rect[0] - 0.10).abs() < 0.000_001);
        assert!((rect[1] - 0.40).abs() < 0.000_001);
        assert!((rect[2] - 0.40).abs() < 0.000_001);
        assert!((rect[3] - 0.80).abs() < 0.000_001);
    }

    #[test]
    fn placement_default_has_positive_size() {
        let [_, _, w, h] = default_rect_for_placement(default_placement_index());
        assert!(w > 0.0);
        assert!(h > 0.0);
    }

    #[test]
    fn placement_labels_are_human_readable() {
        assert!(
            PLACEMENTS
                .iter()
                .all(|entry| !entry.ui_label.contains('_') && entry.ui_label.contains('('))
        );
    }

    #[test]
    fn preview_unavailable_status_mentions_fallback() {
        let status = preview_unavailable_status("teste");
        assert!(status.contains("teste"));
        assert!(status.contains("posicao predefinida"));
    }

    #[test]
    fn default_pdfium_candidates_include_repo_path() {
        let repo_path = repo_pdfium_library_path();
        assert!(
            default_pdfium_candidate_paths()
                .iter()
                .any(|path| path == &repo_path)
        );
    }
}
