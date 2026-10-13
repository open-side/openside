use super::*;
use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::{
    CreateAccountSpaceRequest, CreateAccountSpaceResult, DeviceKind, ReloginConnectionRequest,
};

pub(super) fn person(caller: &Caller) -> std::result::Result<(), HttpError> {
    if caller.0.is_some() && caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"));
    }
    Ok(())
}

pub(crate) async fn create(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateAccountSpaceRequest>,
) -> std::result::Result<Json<CreateAccountSpaceResult>, HttpError> {
    person(&caller)?;
    let site = site(&request.site)
        .filter(|site| site == &request.site)
        .ok_or_else(HttpError::invalid_value)?;
    let id = format!("s_{:032x}", rand::random::<u128>());
    let row = {
        let _gate = shared.connections.gate.lock().await;
        shared
            .connections
            .record(Record {
                site: &site,
                keys: &[],
                source: ConnectionSource::Login,
                status: ConnectionStatus::NeedsLogin,
                identity: None,
                identity_source: ConnectionIdentitySource::None,
                home: Some(&format!("https://{site}/")),
                session: None,
                agent: "chief",
                vault_item: None,
                account_space: Some(&id),
            })
            .map_err(|_| HttpError::internal())?
    };
    let result = api::relogin(
        State(shared.clone()),
        Extension(caller),
        Path(row.id.clone()),
        Json(ReloginConnectionRequest::default()),
    )
    .await;
    match result {
        Ok(Json(result)) => Ok(Json(CreateAccountSpaceResult {
            space_id: id,
            connection_id: Some(row.id),
            result,
        })),
        Err(error) => {
            let _gate = shared.registry.browser.gate.lock().await;
            let root = DataPath::AccountSpaces.path(&shared.config.data_dir);
            if shared
                .registry
                .browser
                .accounts
                .revoke(&root, &id)
                .await
                .is_ok()
            {
                let _ = shared.connections.remove(&row, false, false);
            }
            Err(error)
        }
    }
}

pub(crate) async fn delete(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> std::result::Result<StatusCode, HttpError> {
    person(&caller)?;
    let _browser_gate = shared.registry.browser.gate.lock().await;
    let _gate = shared.connections.gate.lock().await;
    let root = DataPath::AccountSpaces.path(&shared.config.data_dir);
    openside_browser::account_spaces::AccountSpaces::profile(&root, &id)
        .map_err(|_| HttpError::invalid_value())?;
    shared
        .registry
        .browser
        .accounts
        .revoke(&root, &id)
        .await
        .map_err(|_| {
            HttpError(
                StatusCode::SERVICE_UNAVAILABLE,
                "connection_browser_unavailable",
            )
        })?;
    for row in shared
        .connections
        .list()
        .map_err(|_| HttpError::internal())?
        .into_iter()
        .filter(|row| row.account_space.as_deref() == Some(&id))
    {
        shared
            .connections
            .remove(&row, true, false)
            .map_err(|_| HttpError::internal())?;
    }
    Ok(StatusCode::NO_CONTENT)
}
