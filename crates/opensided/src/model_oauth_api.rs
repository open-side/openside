use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    model_oauth::Error,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_core::MessageAuthorKind;
use openside_proto::{
    CreateOAuthAccountRequest, OAuthAccount, OAuthAccountList, OAuthInputRequest,
    OAuthProviderList, OAuthTransaction, ReloginOAuthAccountRequest, UpdateOAuthAccountRequest,
};
use zeroize::Zeroizing;

fn error(error: Error) -> HttpError {
    match error {
        Error::ProviderUnknown => HttpError(StatusCode::BAD_REQUEST, "oauth_provider_unknown"),
        Error::FlowUnsupported => HttpError(StatusCode::BAD_REQUEST, "oauth_flow_unsupported"),
        Error::InvalidValue => HttpError::invalid_value(),
        Error::ManualInputNotAllowed => {
            HttpError(StatusCode::CONFLICT, "oauth_manual_input_not_allowed")
        }
        Error::TransactionFinished => HttpError(StatusCode::CONFLICT, "oauth_transaction_finished"),
        Error::TransactionUnknown => HttpError(StatusCode::NOT_FOUND, "oauth_transaction_unknown"),
        Error::Busy => HttpError(StatusCode::CONFLICT, "oauth_busy"),
        Error::UnknownAccount => HttpError(StatusCode::NOT_FOUND, "unknown_account"),
        Error::VaultUnavailable => HttpError(StatusCode::CONFLICT, "vault_unavailable"),
    }
}

/// Sign-in hands out account access, so only a person's own device may drive it.
fn person(s: &Shared, caller: &Caller, author: MessageAuthorKind) -> Result<(), HttpError> {
    crate::models_api::person(caller)?;
    if matches!(
        author,
        MessageAuthorKind::AgentClient | MessageAuthorKind::Channel
    ) {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"));
    }
    crate::models_api::authorized(s, caller)
}

pub(crate) async fn providers(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Result<Json<OAuthProviderList>, HttpError> {
    person(&s, &caller, author)?;
    Ok(Json(OAuthProviderList {
        providers: crate::model_oauth::providers(),
    }))
}

pub(crate) async fn list(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Result<Json<OAuthAccountList>, HttpError> {
    person(&s, &caller, author)?;
    let accounts = s.model_oauth.list(&s).await.map_err(error)?;
    Ok(Json(OAuthAccountList { accounts }))
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((provider, id)): Path<(String, String)>,
) -> Result<Json<OAuthAccount>, HttpError> {
    person(&s, &caller, author)?;
    s.model_oauth
        .get(&s, &provider, &id)
        .await
        .map(Json)
        .map_err(error)
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(provider): Path<String>,
    Json(request): Json<CreateOAuthAccountRequest>,
) -> Result<(StatusCode, Json<OAuthTransaction>), HttpError> {
    person(&s, &caller, author)?;
    tokio::spawn(async move {
        // Materialise default pools first so a later `join_pool` appends to them.
        crate::model_accounts::initialize(&s).await?;
        crate::models_api::authorized(&s, &caller)?;
        let service = s.model_oauth.clone();
        let transaction = service
            .start(
                &s,
                caller.0.clone(),
                &provider,
                None,
                Some(&request.label),
                request.join_pool,
                request.method,
            )
            .await
            .map_err(error)?;
        Ok((StatusCode::ACCEPTED, Json(transaction)))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn relogin(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((provider, id)): Path<(String, String)>,
    Json(request): Json<ReloginOAuthAccountRequest>,
) -> Result<(StatusCode, Json<OAuthTransaction>), HttpError> {
    person(&s, &caller, author)?;
    let service = s.model_oauth.clone();
    let transaction = service
        .start(
            &s,
            caller.0.clone(),
            &provider,
            Some(&id),
            None,
            false,
            request.method,
        )
        .await
        .map_err(error)?;
    Ok((StatusCode::ACCEPTED, Json(transaction)))
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((provider, id)): Path<(String, String)>,
    Json(request): Json<UpdateOAuthAccountRequest>,
) -> Result<Json<OAuthAccount>, HttpError> {
    person(&s, &caller, author)?;
    tokio::spawn(async move {
        let account = s
            .model_oauth
            .update(
                &s,
                &provider,
                &id,
                request.label.as_deref(),
                request.enabled,
                request.reserve_percent,
            )
            .await
            .map_err(error)?;
        Ok(Json(account))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn remove(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((provider, id)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    person(&s, &caller, author)?;
    tokio::spawn(async move {
        crate::models_api::authorized(&s, &caller)?;
        s.model_oauth
            .remove(&s, &provider, &id)
            .await
            .map_err(error)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn transaction(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<Json<OAuthTransaction>, HttpError> {
    person(&s, &caller, author)?;
    s.model_oauth
        .transaction(caller.0.as_deref(), &id)
        .map(Json)
        .map_err(error)
}

pub(crate) async fn input(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
    Json(mut request): Json<OAuthInputRequest>,
) -> Result<(StatusCode, Json<OAuthTransaction>), HttpError> {
    person(&s, &caller, author)?;
    let code = Zeroizing::new(std::mem::take(&mut request.code));
    s.model_oauth
        .input(caller.0.as_deref(), &id, code)
        .await
        .map(|transaction| (StatusCode::ACCEPTED, Json(transaction)))
        .map_err(error)
}

pub(crate) async fn cancel(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    person(&s, &caller, author)?;
    s.model_oauth
        .cancel(caller.0.as_deref(), &id)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(error)
}
