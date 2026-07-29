use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::LimitrError;
use crate::app_server::AppServer;

#[derive(Clone, Copy)]
pub(crate) struct MonitorPolicy {
    pub(crate) response_timeout: Duration,
    pub(crate) retry_initial: Duration,
    pub(crate) retry_max: Duration,
}

pub(crate) enum MonitorEvent {
    Snapshot(LimitSnapshot),
    Error(String),
}

pub(crate) fn monitor_profile(
    codex_home: PathBuf,
    reconcile_interval: Duration,
    policy: MonitorPolicy,
    events: Sender<MonitorEvent>,
    stop: Receiver<()>,
) {
    let mut backoff = policy.retry_initial;
    loop {
        let mut observed = false;
        match monitor_profile_until_stopped(
            codex_home.clone(),
            reconcile_interval,
            policy.response_timeout,
            &events,
            &stop,
            &mut observed,
        ) {
            Ok(()) => return,
            Err(error) => {
                let retryable = error.is_retryable();
                let detail = if retryable {
                    format!("{error}; retrying")
                } else {
                    error.to_string()
                };
                if events.send(MonitorEvent::Error(detail)).is_err() || !retryable {
                    return;
                }
                match stop.recv_timeout(backoff) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                backoff = if observed {
                    policy.retry_initial
                } else {
                    backoff.saturating_mul(2).min(policy.retry_max)
                };
            }
        }
    }
}

fn monitor_profile_until_stopped(
    codex_home: PathBuf,
    reconcile_interval: Duration,
    response_timeout: Duration,
    events: &Sender<MonitorEvent>,
    stop: &Receiver<()>,
    observed: &mut bool,
) -> Result<(), LimitrError> {
    let mut app_server = AppServer::start(codex_home, response_timeout)?;
    let Some(mut notification_pending) = app_server.initialize_until_stopped(stop)? else {
        return Ok(());
    };
    let Some((identity, notification_during_identity)): Option<(AccountReadResult, _)> = app_server
        .request_until_stopped(1, "account/read", json!({ "refreshToken": false }), stop)?
    else {
        return Ok(());
    };
    notification_pending |= notification_during_identity;
    let identity = identity.require_chatgpt()?;
    let Some((rate_limits, notification_during_limits)): Option<(RateLimitsReadResult, _)> =
        app_server.request_until_stopped(2, "account/rateLimits/read", json!({}), stop)?
    else {
        return Ok(());
    };
    notification_pending |= notification_during_limits;
    events
        .send(MonitorEvent::Snapshot(LimitSnapshot {
            identity: Some(identity),
            rate_limits,
        }))
        .map_err(|_| LimitrError::MonitorClosed)?;
    *observed = true;

    let mut request_id = 3;
    let mut next_reconciliation = if notification_pending {
        Instant::now()
    } else {
        Instant::now() + reconcile_interval
    };
    loop {
        if stop.try_recv().is_ok() {
            return Ok(());
        }
        let now = Instant::now();
        let notification = app_server.poll_message(
            next_reconciliation
                .saturating_duration_since(now)
                .min(Duration::from_millis(100)),
        )?;
        let should_reconcile = notification.as_ref().is_some_and(|message| {
            message.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated")
        }) || Instant::now() >= next_reconciliation;
        if !should_reconcile {
            continue;
        }

        let Some((rate_limits, notification_pending)) = app_server.request_until_stopped(
            request_id,
            "account/rateLimits/read",
            json!({}),
            stop,
        )?
        else {
            return Ok(());
        };
        request_id += 1;
        next_reconciliation = if notification_pending {
            Instant::now()
        } else {
            Instant::now() + reconcile_interval
        };
        events
            .send(MonitorEvent::Snapshot(LimitSnapshot {
                identity: None,
                rate_limits,
            }))
            .map_err(|_| LimitrError::MonitorClosed)?;
    }
}

pub(crate) fn observe_profile(
    codex_home: &Path,
    response_timeout: Duration,
) -> Result<LimitSnapshot, LimitrError> {
    let mut app_server = AppServer::start(codex_home.to_path_buf(), response_timeout)?;
    app_server.initialize()?;
    let identity: AccountReadResult =
        app_server.request(1, "account/read", json!({ "refreshToken": false }))?;
    let identity = identity.require_chatgpt()?;
    let rate_limits: RateLimitsReadResult =
        app_server.request(2, "account/rateLimits/read", json!({}))?;

    Ok(LimitSnapshot {
        identity: Some(identity),
        rate_limits,
    })
}

pub(crate) struct LimitSnapshot {
    pub(crate) identity: Option<AccountIdentity>,
    pub(crate) rate_limits: RateLimitsReadResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountReadResult {
    account: Option<AccountIdentity>,
    #[allow(dead_code)]
    requires_openai_auth: bool,
}

impl AccountReadResult {
    fn require_chatgpt(self) -> Result<AccountIdentity, LimitrError> {
        let identity = self.account.ok_or(LimitrError::Unauthenticated)?;
        match identity.authentication {
            AccountAuthentication::ChatGpt => Ok(identity),
            AccountAuthentication::ApiKey => Err(LimitrError::UnsupportedAuthentication("API-key")),
            AccountAuthentication::AmazonBedrock => {
                Err(LimitrError::UnsupportedAuthentication("Bedrock"))
            }
            AccountAuthentication::Other => {
                Err(LimitrError::UnsupportedAuthentication("Non-ChatGPT"))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountIdentity {
    #[serde(rename = "type")]
    authentication: AccountAuthentication,
    pub(crate) email: Option<String>,
    pub(crate) plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
enum AccountAuthentication {
    #[serde(rename = "chatgpt")]
    ChatGpt,
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "amazonBedrock")]
    AmazonBedrock,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimitsReadResult {
    pub(crate) rate_limits: LimitBucket,
    pub(crate) rate_limits_by_limit_id: Option<BTreeMap<String, LimitBucket>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LimitBucket {
    pub(crate) limit_id: Option<String>,
    pub(crate) limit_name: Option<String>,
    pub(crate) primary: Option<QuotaWindow>,
    pub(crate) secondary: Option<QuotaWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotaWindow {
    pub(crate) used_percent: f64,
    pub(crate) window_duration_mins: u64,
    pub(crate) resets_at: i64,
}
