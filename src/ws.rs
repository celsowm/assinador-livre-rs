use crate::{
    application::{AppService, AppServiceError},
    contracts::{self, BatchFileInput, CertSelectionRequest, VisibleSignatureRequest},
    logger,
};
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
use tokio::{
    sync::oneshot,
    time::{Instant, timeout},
};
use uuid::Uuid;
use warp::{
    Filter, Reply,
    filters::path::FullPath,
    http::StatusCode,
    ws::{Message, WebSocket, Ws},
};

const MAX_BASE64_SIZE: usize = 20 * 1024 * 1024;
const AUTH_TIMEOUT_SECS: u64 = 3;
const SIGN_TIMEOUT_SECS: u64 = 120;
const PLAYGROUND_HTML: &str = include_str!("../assets/playground.html");

pub struct WsServerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl WsServerHandle {
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join_handle.take() {
            let _ = join.join();
        }
    }
}

pub fn spawn_server(service: Arc<AppService>) -> Result<WsServerHandle> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();

    let join = thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                logger::error(format!("Falha ao criar runtime tokio do WS: {e:#}"));
                let _ = ready_tx.send(Err(format!("Falha ao criar runtime tokio: {e:#}")));
                return;
            }
        };

        if let Err(e) = rt.block_on(run_server(service, shutdown_rx, ready_tx)) {
            logger::error(format!("Servidor WS encerrado com erro: {e:#}"));
        }
    });

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(anyhow::anyhow!(err));
        }
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Timeout aguardando inicializacao do servidor WebSocket"
            ));
        }
    }

    Ok(WsServerHandle {
        shutdown_tx: Some(shutdown_tx),
        join_handle: Some(join),
    })
}

async fn run_server(
    service: Arc<AppService>,
    shutdown_rx: oneshot::Receiver<()>,
    ready_tx: mpsc::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let bind_addr = format!("{}:{}", service.config.ws_host, service.config.ws_port);
    let socket_addr: SocketAddr = match bind_addr.parse() {
        Ok(addr) => addr,
        Err(err) => {
            let msg = format!("Endereco de bind invalido ({bind_addr}): {err:#}");
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(anyhow::anyhow!(msg));
        }
    };

    let expected_path = service.config.normalized_ws_path();
    let allowed_origins = service.config.normalized_allowed_origins();
    let local_origins = default_local_origins(&service.config.ws_host, service.config.ws_port);

    let playground_route = warp::path("playground")
        .and(warp::path::end())
        .and(warp::get())
        .map(|| {
            warp::reply::with_header(
                warp::reply::html(PLAYGROUND_HTML),
                "Cache-Control",
                "no-store",
            )
        });

    let root_redirect = warp::path::end()
        .and(warp::get())
        .map(|| warp::redirect::temporary(warp::http::Uri::from_static("/playground")));

    let ws_route = warp::path::full()
        .and(warp::ws())
        .and(warp::header::optional::<String>("origin"))
        .and(with_service(service.clone()))
        .and(with_expected_path(expected_path.clone()))
        .and(with_allowed_origins(allowed_origins.clone()))
        .and(with_local_origins(local_origins.clone()))
        .and_then(
            |full: FullPath,
             ws: Ws,
             origin: Option<String>,
             service: Arc<AppService>,
             expected_path: String,
             allowed_origins: Vec<String>,
             local_origins: Vec<String>| async move {
                route_ws(
                    full,
                    ws,
                    origin,
                    service,
                    expected_path,
                    allowed_origins,
                    local_origins,
                )
                .await
            },
        );

    let routes = playground_route.or(root_redirect).or(ws_route).with(
        warp::cors()
            .allow_methods(vec!["GET", "OPTIONS"])
            .allow_headers(vec!["content-type"]),
    );

    let (server_addr, server) =
        warp::serve(routes).bind_with_graceful_shutdown(socket_addr, async move {
            let _ = shutdown_rx.await;
            logger::info("Shutdown do servidor local (HTTP/WS) recebido");
        });

    let _ = ready_tx.send(Ok(()));
    logger::info(format!(
        "Servidor local ouvindo em ws://{}:{}{} e http://{}:{}/playground",
        service.config.ws_host,
        service.config.ws_port,
        expected_path,
        server_addr.ip(),
        server_addr.port()
    ));

    server.await;
    Ok(())
}

async fn route_ws(
    full: FullPath,
    ws: Ws,
    origin: Option<String>,
    service: Arc<AppService>,
    expected_path: String,
    allowed_origins: Vec<String>,
    local_origins: Vec<String>,
) -> std::result::Result<warp::reply::Response, warp::Rejection> {
    if full.as_str() != expected_path {
        return Err(warp::reject::not_found());
    }

    let normalized_origin = origin.as_ref().map(|v| v.trim().to_ascii_lowercase());
    let origin_allowed = normalized_origin
        .as_ref()
        .map(|origin| {
            is_origin_allowed(origin, &allowed_origins)
                || local_origins.iter().any(|candidate| candidate == origin)
        })
        .unwrap_or(false);

    if !origin_allowed {
        let reply = warp::reply::with_status("ORIGIN_NOT_ALLOWED", StatusCode::FORBIDDEN);
        return Ok(reply.into_response());
    }

    let response = ws
        .max_message_size(22 * 1024 * 1024)
        .max_frame_size(22 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            if let Err(err) = handle_socket(socket, service).await {
                logger::warn(format!("Conexao WS encerrada com erro: {err:#}"));
            }
        })
        .into_response();

    Ok(response)
}

async fn handle_socket(ws: WebSocket, service: Arc<AppService>) -> Result<()> {
    logger::info("Cliente WS conectado");

    let (mut sink, mut stream) = ws.split();

    let first_message = match timeout(Duration::from_secs(AUTH_TIMEOUT_SECS), stream.next()).await {
        Ok(Some(Ok(message))) => message,
        _ => {
            send_error(
                &mut sink,
                None,
                "AUTH_REQUIRED",
                "Autenticacao obrigatoria em ate 3s",
            )
            .await?;
            let _ = sink.close().await;
            return Ok(());
        }
    };

    let auth_req = match parse_request_message(first_message) {
        Ok(req) => req,
        Err(_) => {
            send_error(
                &mut sink,
                None,
                "AUTH_REQUIRED",
                "Primeira mensagem invalida para autenticacao",
            )
            .await?;
            let _ = sink.close().await;
            return Ok(());
        }
    };

    let auth_id = auth_req.id.clone();
    if auth_req.action != "auth" {
        send_error(
            &mut sink,
            auth_id,
            "AUTH_REQUIRED",
            "Primeira mensagem deve ser action=auth",
        )
        .await?;
        let _ = sink.close().await;
        return Ok(());
    }

    let auth_payload: AuthPayload = match serde_json::from_value(auth_req.payload) {
        Ok(payload) => payload,
        Err(_) => {
            send_error(
                &mut sink,
                auth_req.id,
                "AUTH_FAILED",
                "Payload auth invalido",
            )
            .await?;
            let _ = sink.close().await;
            return Ok(());
        }
    };

    if auth_payload.token != service.config.ws_token {
        send_error(&mut sink, auth_req.id, "AUTH_FAILED", "Token invalido").await?;
        let _ = sink.close().await;
        return Ok(());
    }

    send_ok(&mut sink, auth_req.id, json!({"status": "authenticated"})).await?;

    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(err) => {
                logger::warn(format!("Erro de leitura WS: {err:#}"));
                break;
            }
        };

        let req = match parse_request_message(msg) {
            Ok(req) => req,
            Err(err) => {
                logger::warn(format!("Mensagem WS invalida: {err:#}"));
                send_error(&mut sink, None, "INVALID_REQUEST", "Mensagem invalida").await?;
                continue;
            }
        };

        match req.action.as_str() {
            "ping" => {
                send_ok(&mut sink, req.id, json!({"pong": true})).await?;
            }
            "list_certificates" => match service.list_certificates() {
                Ok(certs) => {
                    send_ok(&mut sink, req.id, json!({ "certificates": certs })).await?;
                }
                Err(AppServiceError::BackendUnavailable(msg)) => {
                    send_error(
                        &mut sink,
                        req.id,
                        contracts::SIGNING_BACKEND_UNAVAILABLE,
                        &msg,
                    )
                    .await?;
                }
                Err(err) => {
                    send_error(
                        &mut sink,
                        req.id,
                        "SIGNING_FAILED",
                        &format_service_error(&err),
                    )
                    .await?;
                }
            },
            "sign_pdf" => match handle_sign_pdf(req.payload, service.clone()).await {
                Ok(result) => {
                    send_ok(
                        &mut sink,
                        req.id,
                        json!({
                            "signed_pdf_base64": STANDARD.encode(result.signed_pdf),
                            "cert_subject": result.cert_subject,
                            "cert_issuer": result.cert_issuer,
                            "cert_thumbprint": result.cert_thumbprint,
                            "cert_is_hardware_token": result.cert_is_hardware_token,
                            "cert_provider_name": result.cert_provider_name,
                        }),
                    )
                    .await?;
                }
                Err(WsActionError::Busy) => {
                    send_error(
                        &mut sink,
                        req.id,
                        "BUSY",
                        "Ja existe assinatura em andamento",
                    )
                    .await?;
                }
                Err(WsActionError::Invalid(msg)) => {
                    send_error(&mut sink, req.id, "INVALID_REQUEST", &msg).await?;
                }
                Err(WsActionError::Signing(msg)) => {
                    send_error(&mut sink, req.id, "SIGNING_FAILED", &msg).await?;
                }
                Err(WsActionError::BackendUnavailable(msg)) => {
                    send_error(
                        &mut sink,
                        req.id,
                        contracts::SIGNING_BACKEND_UNAVAILABLE,
                        &msg,
                    )
                    .await?;
                }
            },
            "sign_pdfs" => match handle_sign_pdfs(req.payload, service.clone()).await {
                Ok(result) => {
                    send_ok(&mut sink, req.id, result).await?;
                }
                Err(WsActionError::Busy) => {
                    send_error(
                        &mut sink,
                        req.id,
                        "BUSY",
                        "Ja existe assinatura em andamento",
                    )
                    .await?;
                }
                Err(WsActionError::Invalid(msg)) => {
                    send_error(&mut sink, req.id, "INVALID_REQUEST", &msg).await?;
                }
                Err(WsActionError::Signing(msg)) => {
                    send_error(&mut sink, req.id, "SIGNING_FAILED", &msg).await?;
                }
                Err(WsActionError::BackendUnavailable(msg)) => {
                    send_error(
                        &mut sink,
                        req.id,
                        contracts::SIGNING_BACKEND_UNAVAILABLE,
                        &msg,
                    )
                    .await?;
                }
            },
            _ => {
                send_error(&mut sink, req.id, "INVALID_REQUEST", "Action nao suportada").await?;
            }
        }
    }

    let _ = sink.close().await;
    Ok(())
}

async fn handle_sign_pdf(
    payload: Value,
    service: Arc<AppService>,
) -> std::result::Result<contracts::WsSignResult, WsActionError> {
    let payload: SignPdfPayload = serde_json::from_value(payload)
        .map_err(|_| WsActionError::Invalid("Payload sign_pdf invalido".to_string()))?;

    if payload.pdf_base64.len() > MAX_BASE64_SIZE {
        return Err(WsActionError::Invalid(format!(
            "pdf_base64 excede limite de {} bytes",
            MAX_BASE64_SIZE
        )));
    }

    let pdf_bytes = STANDARD
        .decode(payload.pdf_base64.as_bytes())
        .map_err(|_| WsActionError::Invalid("pdf_base64 invalido".to_string()))?;
    let cert_selection =
        parse_cert_selection(payload.cert_thumbprint.as_deref(), payload.cert_index)?;

    let permit = service
        .signing_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| WsActionError::Busy)?;

    logger::info("Assinatura iniciada via WebSocket");
    let visible_signature = payload.visible_signature.clone();
    let started = Instant::now();

    let service_for_task = service.clone();
    let signing_task = tokio::task::spawn_blocking(move || {
        service_for_task.sign_single_pdf(&pdf_bytes, visible_signature, cert_selection)
    });
    let (result_tx, result_rx) =
        oneshot::channel::<std::result::Result<contracts::WsSignResult, WsActionError>>();

    tokio::spawn(async move {
        let outcome = signing_task
            .await
            .map_err(|err| WsActionError::Signing(format!("Task de assinatura falhou: {err:#}")))
            .and_then(|res| res.map_err(map_service_error));
        let _ = result_tx.send(outcome);
        drop(permit);
    });

    match timeout(Duration::from_secs(SIGN_TIMEOUT_SECS), result_rx).await {
        Ok(Ok(result)) => {
            logger::info(format!(
                "Assinatura WS concluida em {} ms",
                started.elapsed().as_millis()
            ));
            result
        }
        Ok(Err(_)) => Err(WsActionError::Signing(
            "Falha ao receber retorno da assinatura".to_string(),
        )),
        Err(_) => Err(WsActionError::Signing("Timeout na assinatura".to_string())),
    }
}

async fn handle_sign_pdfs(
    payload: Value,
    service: Arc<AppService>,
) -> std::result::Result<Value, WsActionError> {
    let payload: SignPdfsPayload = serde_json::from_value(payload)
        .map_err(|_| WsActionError::Invalid("Payload sign_pdfs invalido".to_string()))?;

    if payload.files.is_empty() {
        return Err(WsActionError::Invalid(
            "Lista de arquivos vazia".to_string(),
        ));
    }
    let cert_selection =
        parse_cert_selection(payload.cert_thumbprint.as_deref(), payload.cert_index)?;

    let mut inputs = Vec::with_capacity(payload.files.len());
    for entry in &payload.files {
        if entry.pdf_base64.len() > MAX_BASE64_SIZE {
            return Err(WsActionError::Invalid(format!(
                "pdf_base64 do arquivo '{}' excede limite de {} bytes",
                entry.filename.as_deref().unwrap_or("?"),
                MAX_BASE64_SIZE
            )));
        }

        let pdf_bytes = STANDARD.decode(entry.pdf_base64.as_bytes()).map_err(|_| {
            WsActionError::Invalid(format!(
                "pdf_base64 invalido no arquivo '{}'",
                entry.filename.as_deref().unwrap_or("?")
            ))
        })?;

        inputs.push(BatchFileInput {
            filename: entry.filename.clone().unwrap_or_default(),
            pdf_bytes,
            visible_signature: entry.visible_signature.clone(),
        });
    }

    let permit = service
        .signing_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| WsActionError::Busy)?;

    logger::info(format!(
        "Assinatura em lote iniciada via WebSocket ({} arquivo(s))",
        inputs.len()
    ));

    let started = Instant::now();

    let service_for_task = service.clone();
    let signing_task = tokio::task::spawn_blocking(move || {
        service_for_task.sign_batch_pdfs(inputs, cert_selection)
    });
    let (result_tx, result_rx) =
        oneshot::channel::<std::result::Result<crate::contracts::BatchSignResult, WsActionError>>();

    tokio::spawn(async move {
        let outcome = signing_task
            .await
            .map_err(|err| WsActionError::Signing(format!("Task de assinatura falhou: {err:#}")))
            .and_then(|res| res.map_err(map_service_error));
        let _ = result_tx.send(outcome);
        drop(permit);
    });

    let batch_result = match timeout(Duration::from_secs(SIGN_TIMEOUT_SECS), result_rx).await {
        Ok(Ok(result)) => {
            logger::info(format!(
                "Assinatura em lote WS concluida em {} ms",
                started.elapsed().as_millis()
            ));
            result?
        }
        Ok(Err(_)) => {
            return Err(WsActionError::Signing(
                "Falha ao receber retorno da assinatura em lote".to_string(),
            ));
        }
        Err(_) => {
            return Err(WsActionError::Signing(
                "Timeout na assinatura em lote".to_string(),
            ));
        }
    };

    let files_json: Vec<Value> = batch_result
        .files
        .into_iter()
        .map(|f| {
            if f.ok {
                json!({
                    "filename": f.filename,
                    "ok": true,
                    "signed_pdf_base64": STANDARD.encode(f.signed_pdf.unwrap_or_default()),
                })
            } else {
                json!({
                    "filename": f.filename,
                    "ok": false,
                    "error": f.error.unwrap_or_default(),
                })
            }
        })
        .collect();

    Ok(json!({
        "files": files_json,
        "cert_subject": batch_result.cert_subject,
        "cert_issuer": batch_result.cert_issuer,
        "cert_thumbprint": batch_result.cert_thumbprint,
        "cert_is_hardware_token": batch_result.cert_is_hardware_token,
        "cert_provider_name": batch_result.cert_provider_name,
    }))
}

fn parse_cert_selection(
    cert_thumbprint: Option<&str>,
    cert_index: Option<usize>,
) -> std::result::Result<Option<CertSelectionRequest>, WsActionError> {
    if let Some(index) = cert_index {
        if index == 0 {
            return Err(WsActionError::Invalid(
                "cert_index invalido: valores aceitos comecam em 1".to_string(),
            ));
        }
    }

    let thumbprint = match cert_thumbprint {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(WsActionError::Invalid(
                    "cert_thumbprint invalido: nao pode ser vazio".to_string(),
                ));
            }
            Some(trimmed.to_string())
        }
        None => None,
    };

    if thumbprint.is_none() && cert_index.is_none() {
        return Ok(None);
    }

    Ok(Some(CertSelectionRequest {
        thumbprint,
        index: cert_index,
    }))
}

fn map_service_error(err: AppServiceError) -> WsActionError {
    match err {
        AppServiceError::Invalid(msg) => WsActionError::Invalid(msg),
        AppServiceError::Signing(msg) => WsActionError::Signing(msg),
        AppServiceError::BackendUnavailable(msg) => WsActionError::BackendUnavailable(msg),
    }
}

fn format_service_error(err: &AppServiceError) -> String {
    match err {
        AppServiceError::Invalid(msg)
        | AppServiceError::Signing(msg)
        | AppServiceError::BackendUnavailable(msg) => msg.clone(),
    }
}

fn parse_request_message(message: Message) -> Result<ClientRequest> {
    if message.is_close() {
        return bail_invalid("Conexao encerrada");
    }
    if message.is_ping() || message.is_pong() {
        return bail_invalid("Frame de controle nao esperado");
    }
    if message.is_binary() {
        return bail_invalid("Mensagem binaria nao suportada");
    }
    if !message.is_text() {
        return bail_invalid("Tipo de mensagem nao suportado");
    }

    let text = message
        .to_str()
        .map_err(|_| anyhow::anyhow!("Mensagem de texto invalida"))?;

    if text.len() > 22 * 1024 * 1024 {
        return bail_invalid("Mensagem excede limite maximo permitido");
    }

    let req: ClientRequest = serde_json::from_str(text).context("JSON invalido")?;

    if req.action.trim().is_empty() {
        return bail_invalid("Campo action obrigatorio");
    }

    Ok(req)
}

pub fn is_origin_allowed(origin: &str, allowed_origins: &[String]) -> bool {
    let normalized = origin.trim().to_ascii_lowercase();
    allowed_origins.iter().any(|allowed| allowed == &normalized)
}

fn default_local_origins(host: &str, port: u16) -> Vec<String> {
    let mut values = vec![
        format!("http://{host}:{port}").to_ascii_lowercase(),
        format!("http://localhost:{port}").to_ascii_lowercase(),
        format!("http://127.0.0.1:{port}").to_ascii_lowercase(),
    ];
    values.sort();
    values.dedup();
    values
}

fn with_service(
    service: Arc<AppService>,
) -> impl Filter<Extract = (Arc<AppService>,), Error = Infallible> + Clone {
    warp::any().map(move || service.clone())
}

fn with_expected_path(
    expected_path: String,
) -> impl Filter<Extract = (String,), Error = Infallible> + Clone {
    warp::any().map(move || expected_path.clone())
}

fn with_allowed_origins(
    allowed_origins: Vec<String>,
) -> impl Filter<Extract = (Vec<String>,), Error = Infallible> + Clone {
    warp::any().map(move || allowed_origins.clone())
}

fn with_local_origins(
    local_origins: Vec<String>,
) -> impl Filter<Extract = (Vec<String>,), Error = Infallible> + Clone {
    warp::any().map(move || local_origins.clone())
}

async fn send_ok(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    id: Option<String>,
    result: Value,
) -> Result<()> {
    let response = ServerSuccess {
        id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        ok: true,
        result,
    };

    let text = serde_json::to_string(&response)?;
    sink.send(Message::text(text)).await?;
    Ok(())
}

async fn send_error(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    id: Option<String>,
    code: &str,
    message: &str,
) -> Result<()> {
    let response = ServerError {
        id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        ok: false,
        error: ErrorBody {
            code: code.to_string(),
            message: message.to_string(),
        },
    };

    let text = serde_json::to_string(&response)?;
    sink.send(Message::text(text)).await?;
    Ok(())
}

fn bail_invalid<T>(message: &str) -> Result<T> {
    Err(anyhow::anyhow!("{message}"))
}

#[derive(Debug, Deserialize)]
struct ClientRequest {
    pub id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
struct AuthPayload {
    token: String,
}

#[derive(Debug, Deserialize)]
struct SignPdfPayload {
    #[allow(dead_code)]
    filename: Option<String>,
    pdf_base64: String,
    visible_signature: Option<VisibleSignatureRequest>,
    cert_thumbprint: Option<String>,
    cert_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SignPdfsPayload {
    files: Vec<SignPdfFileEntry>,
    cert_thumbprint: Option<String>,
    cert_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SignPdfFileEntry {
    filename: Option<String>,
    pdf_base64: String,
    visible_signature: Option<VisibleSignatureRequest>,
}

#[derive(Serialize)]
struct ServerSuccess {
    id: String,
    ok: bool,
    result: Value,
}

#[derive(Serialize)]
struct ServerError {
    id: String,
    ok: bool,
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    message: String,
}

#[derive(Debug)]
enum WsActionError {
    Busy,
    Invalid(String),
    Signing(String),
    BackendUnavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_must_match_exact_list() {
        let allowed = vec!["http://localhost:3000".to_string()];
        assert!(is_origin_allowed("http://localhost:3000", &allowed));
        assert!(!is_origin_allowed("http://localhost:5173", &allowed));
    }

    #[test]
    fn local_origin_list_has_defaults() {
        let values = default_local_origins("127.0.0.1", 45890);
        assert!(values.contains(&"http://127.0.0.1:45890".to_string()));
        assert!(values.contains(&"http://localhost:45890".to_string()));
    }

    #[test]
    fn invalid_request_without_action_fails() {
        let msg = Message::text("{\"id\":\"1\",\"payload\":{}}");
        assert!(parse_request_message(msg).is_err());
    }

    #[test]
    fn sign_pdf_payload_accepts_visible_signature() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "visible_signature": {
                "placement": "top_left_horizontal",
                "style": "compact",
                "timezone": "utc"
            }
        }));
        assert!(payload.is_ok());
    }

    #[test]
    fn sign_pdf_payload_accepts_without_visible_signature() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj"
        }));
        assert!(payload.is_ok());
    }

    #[test]
    fn sign_pdf_payload_rejects_invalid_placement() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "visible_signature": {
                "placement": "centro"
            }
        }));
        assert!(payload.is_err());
    }

    #[test]
    fn sign_pdf_payload_defaults_visible_signature_options() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "visible_signature": {
                "placement": "top_left_horizontal"
            }
        }))
        .unwrap();

        let visible = payload.visible_signature.unwrap();
        assert_eq!(visible.style, contracts::VisibleSignatureStyle::Default);
        assert_eq!(visible.timezone, contracts::VisibleSignatureTimezone::Local);
    }

    #[test]
    fn sign_pdf_payload_rejects_invalid_timezone() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "visible_signature": {
                "placement": "top_left_horizontal",
                "timezone": "america_sao_paulo"
            }
        }));
        assert!(payload.is_err());
    }

    #[test]
    fn sign_pdf_payload_accepts_cert_thumbprint() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "cert_thumbprint": "AA BB CC"
        }))
        .unwrap();
        assert_eq!(payload.cert_thumbprint.as_deref(), Some("AA BB CC"));
    }

    #[test]
    fn sign_pdf_payload_accepts_cert_index() {
        let payload = serde_json::from_value::<SignPdfPayload>(json!({
            "filename": "arquivo.pdf",
            "pdf_base64": "YWJj",
            "cert_index": 2
        }))
        .unwrap();
        assert_eq!(payload.cert_index, Some(2));
    }

    #[test]
    fn cert_selection_prioritizes_thumbprint_over_index() {
        let selection = parse_cert_selection(Some("A1 B2 C3"), Some(3))
            .unwrap()
            .unwrap();
        assert_eq!(selection.thumbprint.as_deref(), Some("A1 B2 C3"));
        assert_eq!(selection.index, Some(3));
    }

    #[test]
    fn cert_selection_rejects_zero_index() {
        let err = parse_cert_selection(None, Some(0)).unwrap_err();
        match err {
            WsActionError::Invalid(msg) => assert!(msg.contains("cert_index invalido")),
            _ => panic!("erro inesperado"),
        }
    }
}
