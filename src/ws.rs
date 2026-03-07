use crate::{app::SharedState, logger, signer};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::{Response as HttpResponse, StatusCode},
        protocol::{Message, WebSocketConfig},
    },
};
use uuid::Uuid;

const MAX_BASE64_SIZE: usize = 20 * 1024 * 1024;
const AUTH_TIMEOUT_SECS: u64 = 3;
const SIGN_TIMEOUT_SECS: u64 = 120;

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

pub fn spawn_server(state: Arc<SharedState>) -> Result<WsServerHandle> {
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

        if let Err(e) = rt.block_on(run_server(state, shutdown_rx, ready_tx)) {
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
    state: Arc<SharedState>,
    mut shutdown_rx: oneshot::Receiver<()>,
    ready_tx: mpsc::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let bind_addr = format!("{}:{}", state.config.ws_host, state.config.ws_port);
    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            let msg = format!("Falha ao escutar em {bind_addr}: {err:#}");
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(anyhow::anyhow!(msg));
        }
    };
    let _ = ready_tx.send(Ok(()));

    logger::info(format!("WebSocket ouvindo em {}", state.config.endpoint()));

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                logger::info("Shutdown do WebSocket recebido");
                break;
            }
            incoming = listener.accept() => {
                let (stream, addr) = match incoming {
                    Ok(v) => v,
                    Err(err) => {
                        logger::warn(format!("Falha ao aceitar conexao WS: {err:#}"));
                        continue;
                    }
                };

                let conn_state = state.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_connection(stream, conn_state).await {
                        logger::warn(format!("Conexao WS {} encerrada com erro: {err:#}", addr));
                    }
                });
            }
        }
    }

    Ok(())
}

async fn handle_connection(stream: TcpStream, state: Arc<SharedState>) -> Result<()> {
    let expected_path = state.config.normalized_ws_path();
    let allowed_origins = state.config.normalized_allowed_origins();

    let callback =
        move |req: &Request, response: Response| -> std::result::Result<Response, ErrorResponse> {
            if req.uri().path() != expected_path {
                return Err(reject(StatusCode::NOT_FOUND, "INVALID_PATH"));
            }

            let origin = req
                .headers()
                .get("Origin")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_ascii_lowercase());

            match origin {
                Some(origin) if is_origin_allowed(&origin, &allowed_origins) => Ok(response),
                _ => Err(reject(StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED")),
            }
        };

    let ws_cfg = Some(
        WebSocketConfig::default()
            .max_message_size(Some(22 * 1024 * 1024))
            .max_frame_size(Some(22 * 1024 * 1024)),
    );

    let ws_stream = accept_hdr_async_with_config(stream, callback, ws_cfg)
        .await
        .context("Handshake WS rejeitado")?;

    logger::info("Cliente WS conectado");

    let (mut sink, mut stream) = ws_stream.split();

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

    if auth_payload.token != state.config.ws_token {
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
            "sign_pdf" => match handle_sign_pdf(req.payload, state.clone()).await {
                Ok(result) => {
                    send_ok(
                        &mut sink,
                        req.id,
                        json!({
                            "signed_pdf_base64": STANDARD.encode(result.signed_pdf),
                            "cert_subject": result.cert_subject,
                            "cert_issuer": result.cert_issuer,
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
    state: Arc<SharedState>,
) -> std::result::Result<signer::WsSignResult, WsActionError> {
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

    let permit = state
        .signing_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| WsActionError::Busy)?;

    logger::info("Assinatura iniciada via WebSocket");
    let cert_override = state.config.cert_override.clone();
    let verbose = state.verbose;

    let signing_task = tokio::task::spawn_blocking(move || {
        signer::sign_single_pdf_bytes(&pdf_bytes, &cert_override, verbose)
    });
    let (result_tx, result_rx) =
        oneshot::channel::<std::result::Result<signer::WsSignResult, WsActionError>>();

    tokio::spawn(async move {
        let outcome = signing_task
            .await
            .map_err(|err| WsActionError::Signing(format!("Task de assinatura falhou: {err:#}")))
            .and_then(|res| res.map_err(|err| WsActionError::Signing(format!("{err:#}"))));
        let _ = result_tx.send(outcome);
        drop(permit);
    });

    match timeout(Duration::from_secs(SIGN_TIMEOUT_SECS), result_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(WsActionError::Signing(
            "Falha ao receber retorno da assinatura".to_string(),
        )),
        Err(_) => Err(WsActionError::Signing("Timeout na assinatura".to_string())),
    }
}

fn parse_request_message(message: Message) -> Result<ClientRequest> {
    let text = match message {
        Message::Text(text) => text,
        Message::Binary(_) => return bail_invalid("Mensagem binaria nao suportada"),
        Message::Ping(_) | Message::Pong(_) => {
            return bail_invalid("Frame de controle nao esperado")
        }
        Message::Close(_) => return bail_invalid("Conexao encerrada"),
        Message::Frame(_) => return bail_invalid("Frame bruto nao suportado"),
    };

    if text.len() > 22 * 1024 * 1024 {
        return bail_invalid("Mensagem excede limite maximo permitido");
    }

    let req: ClientRequest = serde_json::from_str(text.as_ref()).context("JSON invalido")?;

    if req.action.trim().is_empty() {
        return bail_invalid("Campo action obrigatorio");
    }

    Ok(req)
}

fn reject(status: StatusCode, body: &str) -> ErrorResponse {
    HttpResponse::builder()
        .status(status)
        .body(Some(body.to_string()))
        .expect("falha ao construir resposta HTTP")
}

pub fn is_origin_allowed(origin: &str, allowed_origins: &[String]) -> bool {
    let normalized = origin.trim().to_ascii_lowercase();
    allowed_origins.iter().any(|allowed| allowed == &normalized)
}

async fn send_ok(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
    id: Option<String>,
    result: Value,
) -> Result<()> {
    let response = ServerSuccess {
        id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        ok: true,
        result,
    };

    let text = serde_json::to_string(&response)?;
    sink.send(Message::Text(text.into())).await?;
    Ok(())
}

async fn send_error(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
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
    sink.send(Message::Text(text.into())).await?;
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

enum WsActionError {
    Busy,
    Invalid(String),
    Signing(String),
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
    fn invalid_request_without_action_fails() {
        let msg = Message::Text("{\"id\":\"1\",\"payload\":{}}".to_string().into());
        assert!(parse_request_message(msg).is_err());
    }
}
