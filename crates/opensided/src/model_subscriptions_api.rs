use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    managed_subscriptions::Error,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::{
    CreateSubscriptionAccountRequest, SubscriptionAccount, SubscriptionList, SubscriptionSource,
    UpdateSubscriptionAccountRequest,
};

fn error(error: Error) -> HttpError {
    match error {
        Error::InvalidSource => HttpError(StatusCode::BAD_REQUEST, "invalid_source"),
        Error::InvalidValue => HttpError::invalid_value(),
        Error::NotFound => HttpError(StatusCode::NOT_FOUND, "account_not_found"),
        Error::VaultUnavailable => HttpError(StatusCode::CONFLICT, "vault_unavailable"),
        Error::Stale => HttpError(StatusCode::CONFLICT, "account_changed"),
        Error::AuthUnavailable => HttpError(StatusCode::CONFLICT, "auth_unavailable"),
        Error::Io => HttpError(StatusCode::SERVICE_UNAVAILABLE, "subscription_unavailable"),
    }
}

pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<SubscriptionList>, HttpError> {
    let accounts = s.managed_subscriptions.list(&s).await.map_err(error)?;
    let sources = [("claude-code", "anthropic"), ("codex-cli", "openai-codex")]
        .into_iter()
        .map(|(source, provider)| SubscriptionSource {
            source: source.into(),
            provider: provider.into(),
            installed: accounts
                .iter()
                .any(|account| account.source == source && account.installed),
            accounts: accounts
                .iter()
                .filter(|account| account.source == source)
                .cloned()
                .collect(),
        })
        .collect();
    Ok(Json(SubscriptionList { sources }))
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Path((source, id)): Path<(String, String)>,
) -> Result<Json<SubscriptionAccount>, HttpError> {
    s.managed_subscriptions
        .get(&s, &source, &id)
        .await
        .map(Json)
        .map_err(error)
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(source): Path<String>,
    Json(request): Json<CreateSubscriptionAccountRequest>,
) -> Result<(StatusCode, Json<SubscriptionAccount>), HttpError> {
    crate::models_api::person(&caller)?;
    tokio::spawn(async move {
        crate::models_api::authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        crate::models_api::authorized(&s, &caller)?;
        let account = s
            .managed_subscriptions
            .create(&s, &source, &request.label)
            .await
            .map_err(error)?;
        Ok((StatusCode::CREATED, Json(account)))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((source, id)): Path<(String, String)>,
    Json(request): Json<UpdateSubscriptionAccountRequest>,
) -> Result<Json<SubscriptionAccount>, HttpError> {
    crate::models_api::person(&caller)?;
    tokio::spawn(async move {
        crate::models_api::authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        crate::models_api::authorized(&s, &caller)?;
        s.managed_subscriptions
            .update(
                &s,
                &source,
                &id,
                request.label.as_deref(),
                request.enabled,
                request.reserve_percent,
            )
            .await
            .map(Json)
            .map_err(error)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn remove(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((source, id)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    crate::models_api::person(&caller)?;
    tokio::spawn(async move {
        crate::models_api::authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        crate::models_api::authorized(&s, &caller)?;
        s.managed_subscriptions
            .remove(&s, &source, &id)
            .await
            .map_err(error)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| HttpError::internal())?
}
