//! Privileged requests never use editable service URLs, browser sessions or
//! relaxed TLS clients. No action is retried, including after app restart.
use crate::credential_vault::{self, SensitiveString};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Webview};

const TARGET: &str = "192.168.1.10:9473";
const ORIGIN: &str = "https://192.168.1.10:9473";
const CONFIRM_TARGET: &str = "192.168.1.10";
const MAX_HISTORY: usize = 50;
const MAX_BODY: usize = 16 * 1024;
const TRACKING_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PowerAction {
    Reboot,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum OperationState {
    Dispatching,
    Uncertain,
    Scheduled,
    Executing,
    Cancelled,
    Failed,
    Interrupted,
    Completed,
    AwaitingReturn,
    TimedOut,
}

impl OperationState {
    fn active(self) -> bool {
        matches!(
            self,
            Self::Dispatching
                | Self::Uncertain
                | Self::Scheduled
                | Self::Executing
                | Self::AwaitingReturn
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    id: String,
    action: PowerAction,
    state: OperationState,
    requested_at: u64,
    execute_at: Option<u64>,
    observed_offline: bool,
    dry_run: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredOperation {
    operation: Operation,
    boot_id: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    enabled: bool,
    certificate_pem: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u8,
    revision: u64,
    preferences: Preferences,
    history: Vec<StoredOperation>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            preferences: Preferences::default(),
            history: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerStatus {
    boot_id: String,
    uptime_seconds: u64,
    dry_run: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    id: String,
    action: PowerAction,
    expires_at: u64,
    dry_run: bool,
}

struct Pending {
    public: Confirmation,
    boot_id: String,
    deadline: Instant,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ConnectionState {
    NotConfigured,
    Disabled,
    Unchecked,
    Online,
    Unreachable,
    Unauthorized,
    InvalidResponse,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    target: &'static str,
    enabled: bool,
    certificate_pem: String,
    credential_stored: bool,
    revision: String,
    connection_state: ConnectionState,
    server_status: Option<ServerStatus>,
    pending_confirmation: Option<Confirmation>,
    active_operation: Option<Operation>,
    history: Vec<Operation>,
    notice: Option<String>,
}

struct Runtime {
    document: Document,
    pending: Option<Pending>,
    server_status: Option<ServerStatus>,
    connection: ConnectionState,
    notice: Option<String>,
}

pub struct ServerControl {
    path: PathBuf,
    // Deny shared access on Windows for the lifetime of this controller. A
    // second host cannot race a journal write or privileged confirmation.
    owner: Option<File>,
    runtime: Mutex<Runtime>,
    operation: futures_util::lock::Mutex<()>,
    inventory_operation: futures_util::lock::Mutex<()>,
}

impl ServerControl {
    pub fn initialize(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|_| "Server control storage is unavailable.")?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let owner = options.open(directory.join("server-control.lock")).ok();
        let path = directory.join("server-control.json");
        let (mut document, mut notice) = match read_document(&path) {
            Ok(document) => (document, None),
            Err(_) => (Document::default(), Some("Saved server control data is invalid; control is disabled. No command was replayed.".into())),
        };
        if owner.is_none() {
            document.preferences.enabled = false;
            notice =
                Some("Another host owns server control, or its storage is unavailable.".into());
        }
        for entry in &mut document.history {
            if entry.operation.state == OperationState::Dispatching {
                entry.operation.state = OperationState::Uncertain;
            }
        }
        Ok(Self {
            path,
            owner,
            runtime: Mutex::new(Runtime {
                document,
                pending: None,
                server_status: None,
                connection: ConnectionState::Unchecked,
                notice,
            }),
            operation: futures_util::lock::Mutex::new(()),
            inventory_operation: futures_util::lock::Mutex::new(()),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Runtime>, String> {
        self.runtime
            .lock()
            .map_err(|_| "Server control state is unavailable.".into())
    }

    pub(crate) fn has_active_operation(&self) -> bool {
        self.lock()
            .map(|state| {
                state
                    .document
                    .history
                    .iter()
                    .any(|entry| entry.operation.state.active())
            })
            .unwrap_or(false)
    }

    fn ensure_owner(&self) -> Result<(), String> {
        if self.owner.is_none() || !cfg!(windows) {
            return Err("Server control requires the primary Windows host.".into());
        }
        Ok(())
    }

    fn persist(&self, document: &Document) -> Result<(), String> {
        self.ensure_owner()?;
        let data = serde_json::to_vec(document).map_err(|_| "Server control data is invalid.")?;
        let temporary = self.path.with_extension("json.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)?;
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            crate::desktop_integration::atomic_replace(&temporary, &self.path)
        })();
        result.map_err(|_| "Server control history/settings could not be saved.".into())
    }

    fn snapshot(&self) -> Result<Snapshot, String> {
        let mut state = self.lock()?;
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.deadline <= Instant::now())
        {
            state.pending = None;
        }
        let stored = credential_vault::inspect_server_control_token();
        let credential_stored = stored.as_ref().copied().unwrap_or(false);
        let preferences = &state.document.preferences;
        let connection = if !preferences.enabled {
            ConnectionState::Disabled
        } else if preferences.certificate_pem.is_empty() || !credential_stored {
            ConnectionState::NotConfigured
        } else {
            state.connection
        };
        Ok(Snapshot {
            target: TARGET,
            enabled: preferences.enabled,
            certificate_pem: preferences.certificate_pem.clone(),
            credential_stored,
            revision: state.document.revision.to_string(),
            connection_state: connection,
            server_status: state.server_status.clone(),
            pending_confirmation: state.pending.as_ref().map(|pending| pending.public.clone()),
            active_operation: state
                .document
                .history
                .iter()
                .find(|entry| entry.operation.state.active())
                .map(|entry| entry.operation.clone()),
            history: state
                .document
                .history
                .iter()
                .map(|entry| entry.operation.clone())
                .collect(),
            notice: state.notice.clone().or_else(|| {
                stored.err().map(|_| {
                    "The server-control credential vault is unavailable. Save the token again."
                        .into()
                })
            }),
        })
    }

    fn transport(&self) -> Result<Transport, String> {
        self.ensure_owner()?;
        let state = self.lock()?;
        if !state.document.preferences.enabled {
            return Err("Server control is disabled.".into());
        }
        let client = build_client(&state.document.preferences.certificate_pem)?;
        drop(state);
        Ok(Transport {
            client,
            credential: credential_vault::read_server_control_token()?,
        })
    }

    /// On-demand, read-only inventory uses the enrolled trust path but never
    /// takes the power-operation gate. Slow discovery cannot delay cancellation.
    /// No caller can choose a URL or increase the power response-size limit.
    pub(crate) async fn read_inventory<T: DeserializeOwned>(&self) -> Result<T, String> {
        let _guard = self
            .inventory_operation
            .try_lock()
            .ok_or("Server inventory discovery is already in progress.")?;
        let revision = self.lock()?.document.revision;
        let transport = self.transport()?;
        let result = transport
            .request_bounded(Method::GET, "/v1/inventory", None, 128 * 1024)
            .await;
        {
            let state = self.lock()?;
            if !state.document.preferences.enabled || state.document.revision != revision {
                return Err("Server enrollment changed during discovery. Try again.".into());
            }
        }
        result.map_err(|error| match error {
            RemoteError::Unauthorized => "The agent rejected inventory authentication.",
            RemoteError::Rejected => "Inventory is unavailable. Update the server agent and install its optional exporter.",
            RemoteError::Unreachable => "The trusted server inventory endpoint could not be reached.",
            RemoteError::Invalid => "The server inventory response is invalid or too large.",
        }.into())
    }

    fn remember_status(&self, result: &Result<AgentStatus, RemoteError>) -> Result<(), String> {
        let mut state = self.lock()?;
        match result {
            Ok(status) => {
                state.connection = ConnectionState::Online;
                state.server_status = Some(status.public());
            }
            Err(error) => {
                state.connection = error.connection();
                state.server_status = None;
            }
        }
        Ok(())
    }

    async fn refresh(&self) -> Result<(), String> {
        let transport = self.transport()?;
        let result = transport.status().await;
        self.remember_status(&result)
    }

    fn save(&self, request: SaveSettingsRequest) -> Result<(), String> {
        self.ensure_owner()?;
        let token = request.token.map(SensitiveString::new);
        if token.is_some() && request.clear_token {
            return Err("Choose either replacing or removing the token.".into());
        }
        if let Some(token) = &token {
            credential_vault::validate_server_control_token(token.expose())?;
        }
        let certificate = request.certificate_pem.trim().to_string();
        if !certificate.is_empty() {
            build_client(&certificate)?;
        }
        if request.enabled
            && (certificate.is_empty()
                || request.clear_token
                || (token.is_none() && !credential_vault::inspect_server_control_token()?))
        {
            return Err("A trusted certificate and a stored token are required before enabling server control.".into());
        }
        let mut state = self.lock()?;
        ensure_idle(&state)?;
        if request.expected_revision != state.document.revision.to_string() {
            return Err("Server control settings changed. Reload them before saving.".into());
        }
        let mut next = state.document.clone();
        next.preferences = Preferences {
            enabled: request.enabled,
            certificate_pem: certificate,
        };
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Server control revision is exhausted.")?;
        // Credentials never enter this document. Changing the vault first is
        // fail-safe: failed persistence may require re-enrollment, not fallback.
        if let Some(token) = token {
            credential_vault::write_server_control_token(token)?;
        }
        if request.clear_token {
            credential_vault::delete_server_control_token()?;
        }
        self.persist(&next)?;
        state.document = next;
        state.server_status = None;
        state.connection = ConnectionState::Unchecked;
        state.notice = None;
        Ok(())
    }

    async fn prepare(&self, action: PowerAction) -> Result<(), String> {
        {
            ensure_idle(&*self.lock()?)?;
        }
        let transport = self.transport()?;
        let result = transport.status().await;
        self.remember_status(&result)?;
        let status = result.map_err(|_| "The trusted server agent could not be verified. Refresh its status before trying again.")?;
        if status.active_operation.is_some() {
            return Err(
                "The server already has an active operation. Wait for it to finish.".into(),
            );
        }
        let confirmation = Pending {
            public: Confirmation {
                id: random_id()?,
                action,
                expires_at: now_ms() + 60_000,
                dry_run: status.dry_run,
            },
            boot_id: status.boot_id,
            deadline: Instant::now() + Duration::from_secs(60),
        };
        let mut state = self.lock()?;
        state.pending = Some(confirmation);
        state.notice = None;
        Ok(())
    }

    async fn confirm(&self, request: ConfirmRequest) -> Result<(), String> {
        let pending = {
            let mut state = self.lock()?;
            // Consume on every attempt; stale or replayed confirmation cannot
            // dispatch a second request, even after a transport failure.
            let pending = state
                .pending
                .take()
                .ok_or("The confirmation has expired. Start again.")?;
            validate_confirmation(&pending, &request, Instant::now())?;
            pending
        };
        let transport = self.transport()?;
        let status_result = transport.status().await;
        self.remember_status(&status_result)?;
        let status =
            status_result.map_err(|_| "The agent is unavailable. No power request was sent.")?;
        if status.boot_id != pending.boot_id
            || status.dry_run != pending.public.dry_run
            || status.active_operation.is_some()
        {
            return Err(
                "The server changed after confirmation. No power request was sent; start again."
                    .into(),
            );
        }
        if Instant::now() >= pending.deadline {
            return Err("The confirmation expired. No power request was sent.".into());
        }
        let entry = StoredOperation {
            operation: Operation {
                id: random_id()?,
                action: pending.public.action,
                state: OperationState::Dispatching,
                requested_at: now_ms(),
                execute_at: None,
                observed_offline: false,
                dry_run: status.dry_run,
            },
            boot_id: status.boot_id,
        };
        {
            let mut state = self.lock()?;
            let mut document = state.document.clone();
            document.history.insert(0, entry.clone());
            document.history.truncate(MAX_HISTORY);
            // This durable intent must succeed BEFORE the only POST.
            self.persist(&document)?;
            state.document = document;
        }
        let payload = serde_json::json!({
            "id": entry.operation.id, "action": entry.operation.action,
            "bootId": entry.boot_id, "dryRun": entry.operation.dry_run,
        });
        let result: Result<AgentOperation, _> = transport
            .request(Method::POST, "/v1/operations", Some(payload))
            .await;
        let mut next = entry;
        match result {
            Ok(operation) if matches_operation(&next, &operation) => {
                apply_agent_operation(&mut next, &operation);
            }
            Ok(_) => {
                next.operation.state = OperationState::Uncertain;
            }
            Err(RemoteError::Rejected) | Err(RemoteError::Unauthorized) => {
                next.operation.state = OperationState::Failed;
            }
            Err(_) => {
                next.operation.state = OperationState::Uncertain;
            }
        }
        self.update_entry(next)?;
        Ok(())
    }

    fn update_entry(&self, next: StoredOperation) -> Result<(), String> {
        let mut state = self.lock()?;
        let Some(entry) = state
            .document
            .history
            .iter_mut()
            .find(|entry| entry.operation.id == next.operation.id)
        else {
            return Err("The operation history is unavailable.".into());
        };
        *entry = next;
        if let Err(error) = self.persist(&state.document) {
            state.notice = Some("Operation history could not be saved. An accepted server action may still run; do not resend it.".into());
            return Err(error);
        }
        Ok(())
    }

    async fn cancel(&self, id: &str) -> Result<(), String> {
        let mut entry = {
            let state = self.lock()?;
            state
                .document
                .history
                .iter()
                .find(|entry| entry.operation.id == id && entry.operation.state.active())
                .cloned()
                .ok_or("There is no matching active operation.")?
        };
        let transport = self.transport()?;
        let result: Result<AgentOperation, _> = transport
            .request(Method::DELETE, &format!("/v1/operations/{id}"), None)
            .await;
        match result {
            Ok(operation)
                if matches_operation(&entry, &operation)
                    && operation.state == AgentOperationState::Cancelled =>
            {
                apply_agent_operation(&mut entry, &operation);
                self.update_entry(entry)
            }
            _ => {
                self.lock()?.notice = Some("Cancellation was not confirmed. The server may still execute the action; refresh or check the server directly.".into());
                Err("Cancellation was not confirmed. The action may still run.".into())
            }
        }
    }

    // Called only by the native watcher; UI polling cannot create power calls.
    async fn poll(&self, app: &AppHandle) -> Result<(), String> {
        let (mut entry, revision) = {
            let state = self.lock()?;
            match state
                .document
                .history
                .iter()
                .find(|entry| entry.operation.state.active())
            {
                Some(entry) if entry.operation.state != OperationState::Dispatching => {
                    (entry.clone(), state.document.revision)
                }
                Some(_) => return Ok(()),
                None => return Ok(()),
            }
        };
        let before = entry.operation.clone();
        // Do not hold the command gate across watcher I/O: cancellation must
        // never wait behind these probes. Merge only against the same revision
        // and unchanged operation, so an in-flight GET cannot undo cancellation.
        let observation = if let Ok(transport) = self.transport() {
            let status_result = transport.status().await;
            let operation_result = if status_result.is_ok() {
                transport
                    .request::<AgentOperation>(
                        Method::GET,
                        &format!("/v1/operations/{}", entry.operation.id),
                        None,
                    )
                    .await
                    .ok()
            } else {
                None
            };
            Some((status_result, operation_result))
        } else {
            None
        };
        let _guard = self.operation.lock().await;
        {
            let state = self.lock()?;
            if state.document.revision != revision
                || !state
                    .document
                    .history
                    .iter()
                    .any(|current| current.operation == before)
            {
                return Ok(());
            }
        }
        if let Some((status_result, operation_result)) = observation {
            self.remember_status(&status_result)?;
            match status_result {
                Err(RemoteError::Unreachable) => {
                    entry.operation.observed_offline = true;
                    entry.operation.state = OperationState::AwaitingReturn;
                }
                Err(_) => {}
                Ok(status) => {
                    if let Some(operation) = operation_result {
                        if matches_operation(&entry, &operation) {
                            // A new boot alone may follow an unrelated reboot
                            // that interrupted this countdown. Require matching
                            // durable execution evidence, not just availability.
                            if verified_return(&entry, &operation, &status.boot_id) {
                                entry.operation.state = OperationState::Completed;
                                self.update_entry(entry)?;
                                notify_returned(app);
                                return Ok(());
                            }
                            apply_agent_operation(&mut entry, &operation);
                        }
                    }
                }
            }
        }
        // Reconcile once even after a long desktop absence, THEN stop bounded
        // tracking. No power request is ever replayed during this recovery.
        if entry.operation.state.active()
            && now_ms().saturating_sub(entry.operation.requested_at) >= TRACKING_MS
        {
            entry.operation.state = OperationState::TimedOut;
            self.lock()?.notice = Some("Tracking timed out. This does not prove shutdown or cancel an accepted action. Check the server before another request.".into());
        }
        if before != entry.operation {
            self.update_entry(entry)?;
        }
        Ok(())
    }
}

fn ensure_idle(state: &Runtime) -> Result<(), String> {
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.deadline > Instant::now())
        || state
            .document
            .history
            .iter()
            .any(|entry| entry.operation.state.active())
    {
        return Err("Finish or dismiss the existing confirmation/operation first.".into());
    }
    Ok(())
}

fn validate_confirmation(
    pending: &Pending,
    request: &ConfirmRequest,
    now: Instant,
) -> Result<(), String> {
    if request.target != CONFIRM_TARGET
        || request.confirmation_id != pending.public.id
        || now >= pending.deadline
    {
        return Err("The confirmation or typed target is invalid or expired. Start again.".into());
    }
    Ok(())
}

fn build_client(pem: &str) -> Result<Client, String> {
    if pem.len() > MAX_BODY
        || !pem.starts_with("-----BEGIN CERTIFICATE-----")
        || !pem.trim_end().ends_with("-----END CERTIFICATE-----")
        || pem.matches("-----BEGIN CERTIFICATE-----").count() != 1
        || pem.contains("PRIVATE KEY")
    {
        return Err(
            "Supply one PEM public certificate for the server; never its private key.".into(),
        );
    }
    let certificate = reqwest::Certificate::from_pem(pem.as_bytes())
        .map_err(|_| "The server certificate is invalid.")?;
    Client::builder()
        .tls_certs_only([certificate])
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .tls_sslkeylogfile(false)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|_| "The trusted server connection could not be configured.".into())
}

struct Transport {
    client: Client,
    credential: credential_vault::OriginBoundCredential,
}

#[derive(Debug)]
enum RemoteError {
    Unreachable,
    Unauthorized,
    Rejected,
    Invalid,
}

impl RemoteError {
    fn connection(&self) -> ConnectionState {
        match self {
            Self::Unreachable => ConnectionState::Unreachable,
            Self::Unauthorized => ConnectionState::Unauthorized,
            _ => ConnectionState::InvalidResponse,
        }
    }
}

impl Transport {
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, RemoteError> {
        self.request_bounded(method, path, body, MAX_BODY).await
    }

    async fn request_bounded<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        maximum_bytes: usize,
    ) -> Result<T, RemoteError> {
        // Every caller uses a fixed path or a validated, locally generated ID.
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.credential.secret()))
                .map_err(|_| RemoteError::Unauthorized)?;
        authorization.set_sensitive(true);
        let mut builder = self
            .client
            .request(method, format!("{ORIGIN}{path}"))
            .header(reqwest::header::AUTHORIZATION, authorization)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let mut response = builder.send().await.map_err(|_| RemoteError::Unreachable)?;
        let status = response.status().as_u16();
        if matches!(status, 401 | 403) {
            return Err(RemoteError::Unauthorized);
        }
        if matches!(status, 400 | 409 | 404) {
            return Err(RemoteError::Rejected);
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(RemoteError::Invalid);
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| value.split(';').next() != Some("application/json"))
        {
            return Err(RemoteError::Invalid);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RemoteError::Unreachable)?
        {
            if bytes.len() + chunk.len() > maximum_bytes {
                return Err(RemoteError::Invalid);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteError::Invalid)
    }

    async fn status(&self) -> Result<AgentStatus, RemoteError> {
        let status: AgentStatus = self.request(Method::GET, "/v1/status", None).await?;
        if status.version != 1
            || status.server_id != "personal-hub-server"
            || !valid_boot_id(&status.boot_id)
            || status.uptime_seconds > 3_155_760_000
            || status.active_operation.as_ref().is_some_and(|operation| {
                !operation.valid()
                    || !matches!(
                        operation.state,
                        AgentOperationState::Scheduled | AgentOperationState::Executing
                    )
            })
        {
            return Err(RemoteError::Invalid);
        }
        Ok(status)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentStatus {
    version: u8,
    server_id: String,
    boot_id: String,
    uptime_seconds: u64,
    dry_run: bool,
    active_operation: Option<AgentOperation>,
}

impl AgentStatus {
    fn public(&self) -> ServerStatus {
        ServerStatus {
            boot_id: self.boot_id.clone(),
            uptime_seconds: self.uptime_seconds,
            dry_run: self.dry_run,
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum AgentOperationState {
    Scheduled,
    Executing,
    Cancelled,
    Failed,
    Interrupted,
    Completed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentOperation {
    id: String,
    action: PowerAction,
    state: AgentOperationState,
    requested_at: u64,
    execute_at: u64,
    boot_id: String,
}

impl AgentOperation {
    fn valid(&self) -> bool {
        valid_id(&self.id)
            && valid_boot_id(&self.boot_id)
            && self.execute_at < 9_007_199_254_740_991
            && self.requested_at.checked_add(30_000) == Some(self.execute_at)
    }
}

fn matches_operation(entry: &StoredOperation, remote: &AgentOperation) -> bool {
    remote.valid()
        && entry.operation.id == remote.id
        && entry.operation.action == remote.action
        && entry.boot_id == remote.boot_id
}

fn verified_return(entry: &StoredOperation, remote: &AgentOperation, boot_id: &str) -> bool {
    matches_operation(entry, remote)
        && !entry.operation.dry_run
        && boot_id != entry.boot_id
        && matches!(
            remote.state,
            AgentOperationState::Executing | AgentOperationState::Completed
        )
}

fn apply_agent_operation(entry: &mut StoredOperation, remote: &AgentOperation) {
    entry.operation.execute_at = Some(remote.execute_at);
    entry.operation.state = match remote.state {
        AgentOperationState::Scheduled => OperationState::Scheduled,
        AgentOperationState::Executing => OperationState::Executing,
        AgentOperationState::Cancelled => OperationState::Cancelled,
        AgentOperationState::Failed => OperationState::Failed,
        AgentOperationState::Interrupted => OperationState::Interrupted,
        AgentOperationState::Completed if entry.operation.dry_run => OperationState::Completed,
        // Real completion requires separately observed changed boot identity.
        AgentOperationState::Completed => OperationState::AwaitingReturn,
    };
}

fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_boot_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn read_document(path: &PathBuf) -> Result<Document, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Document::default())
        }
        Err(_) => return Err("Server control data could not be read.".into()),
    };
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Server control data could not be read.")?;
    if bytes.len() > 128 * 1024 {
        return Err("Server control data is too large.".into());
    }
    let document: Document =
        serde_json::from_slice(&bytes).map_err(|_| "Server control data is invalid.")?;
    if document.version != 1
        || document.history.len() > MAX_HISTORY
        || document.preferences.certificate_pem.len() > MAX_BODY
        || document
            .history
            .iter()
            .filter(|entry| entry.operation.state.active())
            .count()
            > 1
        || document.history.iter().any(|entry| {
            !valid_id(&entry.operation.id)
                || !valid_boot_id(&entry.boot_id)
                || entry.operation.requested_at > now_ms().saturating_add(60_000)
                || entry
                    .operation
                    .execute_at
                    .is_some_and(|value| value >= 9_007_199_254_740_991)
        })
    {
        return Err("Server control data is invalid.".into());
    }
    if document.preferences.enabled {
        build_client(&document.preferences.certificate_pem)?;
    }
    Ok(document)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        extern "system" {
            fn BCryptGenRandom(
                algorithm: *mut std::ffi::c_void,
                buffer: *mut u8,
                length: u32,
                flags: u32,
            ) -> i32;
        }
        // SAFETY: valid writable buffer; USE_SYSTEM_PREFERRED_RNG accepts NULL.
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                2,
            )
        } < 0
        {
            return Err("Secure confirmation generation is unavailable.".into());
        }
    }
    #[cfg(not(windows))]
    {
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut bytes))
            .map_err(|_| "Secure confirmation generation is unavailable.")?;
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn authorize_main(label: &str) -> Result<(), String> {
    if label != "main" {
        return Err("Server power control is available only to the trusted main UI.".into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveSettingsRequest {
    enabled: bool,
    certificate_pem: String,
    token: Option<String>,
    clear_token: bool,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareRequest {
    action: PowerAction,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmRequest {
    confirmation_id: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelRequest {
    operation_id: String,
}

#[tauri::command]
pub fn get_server_control_snapshot(
    caller: Webview,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    control.snapshot()
}

#[tauri::command]
pub async fn save_server_control_settings(
    caller: Webview,
    request: SaveSettingsRequest,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    let _guard = control.operation.lock().await;
    control.save(request)?;
    control.snapshot()
}

#[tauri::command]
pub async fn refresh_server_control_status(
    caller: Webview,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    let _guard = control.operation.lock().await;
    control.refresh().await?;
    control.snapshot()
}

#[tauri::command]
pub async fn prepare_server_control_action(
    caller: Webview,
    request: PrepareRequest,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    let _guard = control.operation.lock().await;
    control.prepare(request.action).await?;
    control.snapshot()
}

#[tauri::command]
pub async fn confirm_server_control_action(
    caller: Webview,
    request: ConfirmRequest,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    let _guard = control.operation.lock().await;
    control.confirm(request).await?;
    control.snapshot()
}

#[tauri::command]
pub async fn cancel_server_control_operation(
    caller: Webview,
    request: CancelRequest,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    if !valid_id(&request.operation_id) {
        return Err("Invalid operation ID.".into());
    }
    let _guard = control.operation.lock().await;
    control.cancel(&request.operation_id).await?;
    control.snapshot()
}

#[tauri::command]
pub async fn dismiss_server_control_confirmation(
    caller: Webview,
    control: tauri::State<'_, ServerControl>,
) -> Result<Snapshot, String> {
    authorize_main(caller.label())?;
    let _guard = control.operation.lock().await;
    control.lock()?.pending = None;
    control.snapshot()
}

pub fn setup(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    std::thread::Builder::new()
        .name("personal-hub-server-control".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3));
            let control = app.state::<ServerControl>();
            let _ = tauri::async_runtime::block_on(control.poll(&app));
        })
        .map_err(|_| "The server operation watcher could not start.".to_string())?;
    Ok(())
}

fn notify_returned(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri_plugin_notification::NotificationExt;
        let turkish = crate::appearance_settings::notification_language_is_turkish(app);
        let body = if turkish {
            "Sunucu yeniden çevrimiçi. Yeni açılış kimliği doğrulandı."
        } else {
            "The server is back online. Its new boot identity was verified."
        };
        let _ = app
            .notification()
            .builder()
            .title("Personal Hub")
            .body(body)
            .show();
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_control_rejects_remote_ui_and_arbitrary_targets() {
        assert!(authorize_main("main").is_ok());
        assert!(authorize_main("service-jellyfin").is_err());
        assert!(authorize_main("widget-server").is_err());
        assert!(serde_json::from_str::<PrepareRequest>(
            r#"{"action":"shutdown","target":"localhost"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<PrepareRequest>(r#"{"action":"shell"}"#).is_err());
        assert!(!valid_id("../../etc/shadow"));
        assert!(build_client("-----BEGIN PRIVATE KEY-----").is_err());
    }

    #[test]
    fn server_control_confirmation_is_target_bound_and_expires() {
        let pending = Pending {
            public: Confirmation {
                id: "a".repeat(32),
                action: PowerAction::Reboot,
                expires_at: now_ms() + 60_000,
                dry_run: true,
            },
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
            deadline: Instant::now() + Duration::from_secs(60),
        };
        let mut request = ConfirmRequest {
            confirmation_id: pending.public.id.clone(),
            target: CONFIRM_TARGET.into(),
        };
        assert!(validate_confirmation(&pending, &request, Instant::now()).is_ok());
        assert!(validate_confirmation(&pending, &request, pending.deadline).is_err());
        request.target = "localhost".into();
        assert!(validate_confirmation(&pending, &request, Instant::now()).is_err());
    }

    #[test]
    fn server_control_completion_needs_boot_evidence_except_dry_run() {
        let mut entry = StoredOperation {
            operation: Operation {
                id: "a".repeat(32),
                action: PowerAction::Shutdown,
                state: OperationState::Uncertain,
                requested_at: 1000,
                execute_at: None,
                observed_offline: true,
                dry_run: false,
            },
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
        };
        let mut remote = AgentOperation {
            id: entry.operation.id.clone(),
            action: PowerAction::Shutdown,
            state: AgentOperationState::Completed,
            requested_at: 1000,
            execute_at: 31_000,
            boot_id: entry.boot_id.clone(),
        };
        assert!(matches_operation(&entry, &remote));
        apply_agent_operation(&mut entry, &remote);
        assert_eq!(entry.operation.state, OperationState::AwaitingReturn);
        entry.operation.dry_run = true;
        apply_agent_operation(&mut entry, &remote);
        assert_eq!(entry.operation.state, OperationState::Completed);
        remote.action = PowerAction::Reboot;
        assert!(!matches_operation(&entry, &remote));
        remote.action = PowerAction::Shutdown;
        entry.operation.dry_run = false;
        let next_boot = "11111111-1111-1111-1111-111111111111";
        assert!(verified_return(&entry, &remote, next_boot));
        assert!(!verified_return(&entry, &remote, &entry.boot_id));
        remote.state = AgentOperationState::Interrupted;
        assert!(!verified_return(&entry, &remote, next_boot));
        remote.state = AgentOperationState::Cancelled;
        assert!(!verified_return(&entry, &remote, next_boot));
    }

    #[test]
    fn server_control_ids_use_secure_unique_bytes() {
        let first = random_id().unwrap();
        assert!(valid_id(&first));
        assert_ne!(first, random_id().unwrap());
    }
}
