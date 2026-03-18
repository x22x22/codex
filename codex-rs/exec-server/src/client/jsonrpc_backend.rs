use serde::Serialize;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;

use crate::client_api::RemoteExecServerConnectArgs;
use crate::connection::JsonRpcConnection;
use crate::rpc::RpcCallError;
use crate::rpc::RpcClient;
use crate::rpc::RpcClientEvent;

use super::ExecServerError;

pub(super) struct JsonRpcBackend {
    rpc: RpcClient,
}

impl JsonRpcBackend {
    pub(super) fn connect_stdio<R, W>(stdin: W, stdout: R) -> (Self, mpsc::Receiver<RpcClientEvent>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::connect(JsonRpcConnection::from_stdio(
            stdout,
            stdin,
            "exec-server stdio".to_string(),
        ))
    }

    pub(super) async fn connect_websocket(
        args: &RemoteExecServerConnectArgs,
    ) -> Result<(Self, mpsc::Receiver<RpcClientEvent>), ExecServerError> {
        let websocket_url = args.websocket_url.clone();
        let connect_timeout = args.connect_timeout;
        let (stream, _) = timeout(connect_timeout, connect_async(websocket_url.as_str()))
            .await
            .map_err(|_| ExecServerError::WebSocketConnectTimeout {
                url: websocket_url.clone(),
                timeout: connect_timeout,
            })?
            .map_err(|source| ExecServerError::WebSocketConnect {
                url: websocket_url.clone(),
                source,
            })?;

        Ok(Self::connect(JsonRpcConnection::from_websocket(
            stream,
            format!("exec-server websocket {websocket_url}"),
        )))
    }

    fn connect(connection: JsonRpcConnection) -> (Self, mpsc::Receiver<RpcClientEvent>) {
        let (rpc, events_rx) = RpcClient::new(connection);
        (Self { rpc }, events_rx)
    }

    pub(super) async fn notify<P: Serialize>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), ExecServerError> {
        self.rpc
            .notify(method, params)
            .await
            .map_err(|err| match err.classify() {
                serde_json::error::Category::Io => ExecServerError::Closed,
                serde_json::error::Category::Syntax
                | serde_json::error::Category::Data
                | serde_json::error::Category::Eof => ExecServerError::Json(err),
            })
    }

    pub(super) async fn call<P, T>(&self, method: &str, params: &P) -> Result<T, ExecServerError>
    where
        P: Serialize,
        T: serde::de::DeserializeOwned,
    {
        self.rpc
            .call(method, params)
            .await
            .map_err(|err| match err {
                RpcCallError::Closed => ExecServerError::Closed,
                RpcCallError::Json(err) => ExecServerError::Json(err),
                RpcCallError::Server(error) => ExecServerError::Server {
                    code: error.code,
                    message: error.message,
                },
            })
    }

    #[cfg(test)]
    pub(super) async fn pending_request_count(&self) -> usize {
        self.rpc.pending_request_count().await
    }
}
